//! SM3 intermediate representation: the token stream in Rust types.
//!
//! Everything the tokens carry is kept (control bits, write masks, swizzles, source modifiers,
//! relative addressing), so that the disassembler, the catalog features and the oracle all work
//! on the same data (SR-D3).

use super::ctab::Ctab;
use crate::scan::Stage;

/// Shader version token: stage, major, minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub stage: Stage,
    pub major: u8,
    pub minor: u8,
}

impl Version {
    /// `vs_3_0` / `ps_3_0`.
    pub fn profile(&self) -> String {
        let s = if self.stage == Stage::Vs { "vs" } else { "ps" };
        format!("{s}_{}_{}", self.major, self.minor)
    }
}

/// Register file (`D3DSHADER_PARAM_REGISTER_TYPE`, d3d9types.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RegType {
    /// `r#`
    Temp,
    /// `v#`
    Input,
    /// `c#`
    Const,
    /// `a0` in a vertex shader, `t#` in a pixel shader.
    Addr,
    /// `oPos`, `oFog`, `oPts` (vertex shader, SM1/2).
    RastOut,
    /// `oD#` (vertex shader, SM1/2).
    AttrOut,
    /// `o#` (vertex shader SM3 outputs, `D3DSPR_OUTPUT`).
    Output,
    /// `i#`
    ConstInt,
    /// `oC#`
    ColorOut,
    /// `oDepth`
    DepthOut,
    /// `s#`
    Sampler,
    /// `c#` above 2047.
    Const2,
    Const3,
    Const4,
    /// `b#`
    ConstBool,
    /// `aL`
    Loop,
    /// `h#`
    TempFloat16,
    /// `vPos` (number 0), `vFace` (number 1).
    MiscType,
    /// `l#`
    Label,
    /// `p0`
    Predicate,
}

impl RegType {
    /// From the 5-bit code of the token.
    pub fn from_code(c: u32) -> Option<RegType> {
        use RegType::*;
        Some(match c {
            0 => Temp,
            1 => Input,
            2 => Const,
            3 => Addr,
            4 => RastOut,
            5 => AttrOut,
            6 => Output,
            7 => ConstInt,
            8 => ColorOut,
            9 => DepthOut,
            10 => Sampler,
            11 => Const2,
            12 => Const3,
            13 => Const4,
            14 => ConstBool,
            15 => Loop,
            16 => TempFloat16,
            17 => MiscType,
            18 => Label,
            19 => Predicate,
            _ => return None,
        })
    }

    pub fn code(self) -> u32 {
        use RegType::*;
        match self {
            Temp => 0,
            Input => 1,
            Const => 2,
            Addr => 3,
            RastOut => 4,
            AttrOut => 5,
            Output => 6,
            ConstInt => 7,
            ColorOut => 8,
            DepthOut => 9,
            Sampler => 10,
            Const2 => 11,
            Const3 => 12,
            Const4 => 13,
            ConstBool => 14,
            Loop => 15,
            TempFloat16 => 16,
            MiscType => 17,
            Label => 18,
            Predicate => 19,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reg {
    pub ty: RegType,
    pub num: u32,
}

/// Relative addressing: the index register (`a0` or `aL`) and the component of `a0` that is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rel {
    pub ty: RegType,
    pub num: u32,
    /// 0..=3 for x, y, z, w (`aL` has none; stored as 0).
    pub comp: u8,
}

/// Destination modifier bits (token bits 20-23).
pub const MOD_SAT: u8 = 1;
pub const MOD_PP: u8 = 2;
pub const MOD_CENTROID: u8 = 4;

/// Destination parameter: register, write mask, result modifiers, shift scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dst {
    pub reg: Reg,
    pub rel: Option<Rel>,
    /// Bit 0 = x ... bit 3 = w.
    pub mask: u8,
    /// `MOD_SAT | MOD_PP | MOD_CENTROID`.
    pub mods: u8,
    /// Result shift, signed 4 bits: +1 = `_x2`, -1 = `_d2`.
    pub shift: i8,
}

impl Dst {
    pub fn sat(&self) -> bool {
        self.mods & MOD_SAT != 0
    }
    pub fn pp(&self) -> bool {
        self.mods & MOD_PP != 0
    }
    pub fn centroid(&self) -> bool {
        self.mods & MOD_CENTROID != 0
    }
}

/// Source modifier (`D3DSPSM_*`, token bits 24-27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrcMod {
    None,
    Neg,
    Bias,
    BiasNeg,
    Sign,
    SignNeg,
    Comp,
    X2,
    X2Neg,
    Dz,
    Dw,
    Abs,
    AbsNeg,
    Not,
}

impl SrcMod {
    pub fn from_code(c: u32) -> Option<SrcMod> {
        use SrcMod::*;
        Some(match c {
            0 => None,
            1 => Neg,
            2 => Bias,
            3 => BiasNeg,
            4 => Sign,
            5 => SignNeg,
            6 => Comp,
            7 => X2,
            8 => X2Neg,
            9 => Dz,
            10 => Dw,
            11 => Abs,
            12 => AbsNeg,
            13 => Not,
            _ => return Option::None,
        })
    }

    pub fn code(self) -> u32 {
        use SrcMod::*;
        match self {
            None => 0,
            Neg => 1,
            Bias => 2,
            BiasNeg => 3,
            Sign => 4,
            SignNeg => 5,
            Comp => 6,
            X2 => 7,
            X2Neg => 8,
            Dz => 9,
            Dw => 10,
            Abs => 11,
            AbsNeg => 12,
            Not => 13,
        }
    }
}

/// Source parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Src {
    pub reg: Reg,
    pub rel: Option<Rel>,
    /// Source component for each of x, y, z, w (0..=3).
    pub swizzle: [u8; 4],
    pub modifier: SrcMod,
}

impl Src {
    pub const IDENTITY: [u8; 4] = [0, 1, 2, 3];
}

/// Comparison of `ifc`, `breakc`, `setp` (`D3DSPC_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Eq,
    Ge,
    Lt,
    Ne,
    Le,
}

impl Cmp {
    pub fn from_code(c: u8) -> Option<Cmp> {
        Some(match c {
            1 => Cmp::Gt,
            2 => Cmp::Eq,
            3 => Cmp::Ge,
            4 => Cmp::Lt,
            5 => Cmp::Ne,
            6 => Cmp::Le,
            _ => return None,
        })
    }
    pub fn code(self) -> u8 {
        match self {
            Cmp::Gt => 1,
            Cmp::Eq => 2,
            Cmp::Ge => 3,
            Cmp::Lt => 4,
            Cmp::Ne => 5,
            Cmp::Le => 6,
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Cmp::Gt => "gt",
            Cmp::Eq => "eq",
            Cmp::Ge => "ge",
            Cmp::Lt => "lt",
            Cmp::Ne => "ne",
            Cmp::Le => "le",
        }
    }
}

/// Instruction opcodes of ps/vs 2_0..3_0 (`D3DSHADER_INSTRUCTION_OPCODE_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Opcode {
    Nop,
    Mov,
    Add,
    Sub,
    Mad,
    Mul,
    Rcp,
    Rsq,
    Dp3,
    Dp4,
    Min,
    Max,
    Slt,
    Sge,
    Exp,
    Log,
    Lit,
    Dst,
    Lrp,
    Frc,
    M4x4,
    M4x3,
    M3x4,
    M3x3,
    M3x2,
    Call,
    CallNz,
    Loop,
    Ret,
    EndLoop,
    Label,
    Dcl,
    Pow,
    Crs,
    Sgn,
    Abs,
    Nrm,
    SinCos,
    Rep,
    EndRep,
    If,
    Ifc,
    Else,
    EndIf,
    Break,
    BreakC,
    Mova,
    DefB,
    DefI,
    TexKill,
    /// `texld` / `texldp` / `texldb`, told apart by [`Inst::control`].
    TexLd,
    ExpP,
    LogP,
    Def,
    Cmp,
    Dp2Add,
    Dsx,
    Dsy,
    TexLdd,
    SetP,
    TexLdl,
    BreakP,
}

impl Opcode {
    /// Every opcode, in token-value order.
    pub const ALL: [Opcode; 62] = {
        use Opcode::*;
        [
            Nop, Mov, Add, Sub, Mad, Mul, Rcp, Rsq, Dp3, Dp4, Min, Max, Slt, Sge, Exp, Log, Lit,
            Dst, Lrp, Frc, M4x4, M4x3, M3x4, M3x3, M3x2, Call, CallNz, Loop, Ret, EndLoop, Label,
            Dcl, Pow, Crs, Sgn, Abs, Nrm, SinCos, Rep, EndRep, If, Ifc, Else, EndIf, Break, BreakC,
            Mova, DefB, DefI, TexKill, TexLd, ExpP, LogP, Def, Cmp, Dp2Add, Dsx, Dsy, TexLdd, SetP,
            TexLdl, BreakP,
        ]
    };

    /// Token value (`D3DSIO_*`).
    pub fn code(self) -> u16 {
        use Opcode::*;
        match self {
            Nop => 0,
            Mov => 1,
            Add => 2,
            Sub => 3,
            Mad => 4,
            Mul => 5,
            Rcp => 6,
            Rsq => 7,
            Dp3 => 8,
            Dp4 => 9,
            Min => 10,
            Max => 11,
            Slt => 12,
            Sge => 13,
            Exp => 14,
            Log => 15,
            Lit => 16,
            Dst => 17,
            Lrp => 18,
            Frc => 19,
            M4x4 => 20,
            M4x3 => 21,
            M3x4 => 22,
            M3x3 => 23,
            M3x2 => 24,
            Call => 25,
            CallNz => 26,
            Loop => 27,
            Ret => 28,
            EndLoop => 29,
            Label => 30,
            Dcl => 31,
            Pow => 32,
            Crs => 33,
            Sgn => 34,
            Abs => 35,
            Nrm => 36,
            SinCos => 37,
            Rep => 38,
            EndRep => 39,
            If => 40,
            Ifc => 41,
            Else => 42,
            EndIf => 43,
            Break => 44,
            BreakC => 45,
            Mova => 46,
            DefB => 47,
            DefI => 48,
            TexKill => 65,
            TexLd => 66,
            ExpP => 78,
            LogP => 79,
            Def => 81,
            Cmp => 88,
            Dp2Add => 90,
            Dsx => 91,
            Dsy => 92,
            TexLdd => 93,
            SetP => 94,
            TexLdl => 95,
            BreakP => 96,
        }
    }

    pub fn from_code(c: u16) -> Option<Opcode> {
        Opcode::ALL.into_iter().find(|o| o.code() == c)
    }

    /// Mnemonic as `D3DDisassemble` prints it, without suffixes (`texld` for all three loads).
    pub fn name(self) -> &'static str {
        use Opcode::*;
        match self {
            Nop => "nop",
            Mov => "mov",
            Add => "add",
            Sub => "sub",
            Mad => "mad",
            Mul => "mul",
            Rcp => "rcp",
            Rsq => "rsq",
            Dp3 => "dp3",
            Dp4 => "dp4",
            Min => "min",
            Max => "max",
            Slt => "slt",
            Sge => "sge",
            Exp => "exp",
            Log => "log",
            Lit => "lit",
            Dst => "dst",
            Lrp => "lrp",
            Frc => "frc",
            M4x4 => "m4x4",
            M4x3 => "m4x3",
            M3x4 => "m3x4",
            M3x3 => "m3x3",
            M3x2 => "m3x2",
            Call => "call",
            CallNz => "callnz",
            Loop => "loop",
            Ret => "ret",
            EndLoop => "endloop",
            Label => "label",
            Dcl => "dcl",
            Pow => "pow",
            Crs => "crs",
            Sgn => "sgn",
            Abs => "abs",
            Nrm => "nrm",
            SinCos => "sincos",
            Rep => "rep",
            EndRep => "endrep",
            If => "if",
            Ifc => "if",
            Else => "else",
            EndIf => "endif",
            Break => "break",
            BreakC => "break",
            Mova => "mova",
            DefB => "defb",
            DefI => "defi",
            TexKill => "texkill",
            TexLd => "texld",
            ExpP => "expp",
            LogP => "logp",
            Def => "def",
            Cmp => "cmp",
            Dp2Add => "dp2add",
            Dsx => "dsx",
            Dsy => "dsy",
            TexLdd => "texldd",
            SetP => "setp",
            TexLdl => "texldl",
            BreakP => "breakp",
        }
    }
}

/// `texld` flavour in the control bits (`D3DSI_TEXLD_*`).
pub const TEXLD_PLAIN: u8 = 0;
pub const TEXLD_PROJECT: u8 = 1;
pub const TEXLD_BIAS: u8 = 2;

/// An instruction with operands. `def*` and `dcl` have their own statements.
#[derive(Clone, Debug, PartialEq)]
pub struct Inst {
    pub op: Opcode,
    /// Control bits (token bits 16-23): comparison of `ifc`/`breakc`/`setp`, flavour of `texld`.
    pub control: u8,
    pub dst: Option<Dst>,
    pub src: Vec<Src>,
    /// Predicate source (instruction predicated by `p0`, token bit 28).
    pub pred: Option<Src>,
}

impl Inst {
    /// Comparison of `ifc`, `breakc`, `setp`.
    pub fn cmp(&self) -> Option<Cmp> {
        match self.op {
            Opcode::Ifc | Opcode::BreakC | Opcode::SetP => Cmp::from_code(self.control),
            _ => None,
        }
    }
}

/// Sampler type of `dcl_2d` / `dcl_cube` / `dcl_volume` (`D3DSAMPLER_TEXTURE_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerKind {
    Unknown,
    Tex1d,
    Tex2d,
    Cube,
    Volume,
}

impl SamplerKind {
    pub fn from_code(c: u32) -> SamplerKind {
        match c {
            1 => SamplerKind::Tex1d,
            2 => SamplerKind::Tex2d,
            3 => SamplerKind::Cube,
            4 => SamplerKind::Volume,
            _ => SamplerKind::Unknown,
        }
    }
    pub fn code(self) -> u32 {
        match self {
            SamplerKind::Unknown => 0,
            SamplerKind::Tex1d => 1,
            SamplerKind::Tex2d => 2,
            SamplerKind::Cube => 3,
            SamplerKind::Volume => 4,
        }
    }
}

/// Vertex-element usage of an input/output declaration (`D3DDECLUSAGE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Position,
    BlendWeight,
    BlendIndices,
    Normal,
    PSize,
    TexCoord,
    Tangent,
    Binormal,
    TessFactor,
    PositionT,
    Color,
    Fog,
    Depth,
    Sample,
}

impl Usage {
    pub fn from_code(c: u32) -> Option<Usage> {
        use Usage::*;
        Some(match c {
            0 => Position,
            1 => BlendWeight,
            2 => BlendIndices,
            3 => Normal,
            4 => PSize,
            5 => TexCoord,
            6 => Tangent,
            7 => Binormal,
            8 => TessFactor,
            9 => PositionT,
            10 => Color,
            11 => Fog,
            12 => Depth,
            13 => Sample,
            _ => return None,
        })
    }
    pub fn code(self) -> u32 {
        use Usage::*;
        match self {
            Position => 0,
            BlendWeight => 1,
            BlendIndices => 2,
            Normal => 3,
            PSize => 4,
            TexCoord => 5,
            Tangent => 6,
            Binormal => 7,
            TessFactor => 8,
            PositionT => 9,
            Color => 10,
            Fog => 11,
            Depth => 12,
            Sample => 13,
        }
    }
    /// Name as in `dcl_<name>`.
    pub fn name(self) -> &'static str {
        use Usage::*;
        match self {
            Position => "position",
            BlendWeight => "blendweight",
            BlendIndices => "blendindices",
            Normal => "normal",
            PSize => "psize",
            TexCoord => "texcoord",
            Tangent => "tangent",
            Binormal => "binormal",
            TessFactor => "tessfactor",
            PositionT => "positiont",
            Color => "color",
            Fog => "fog",
            Depth => "depth",
            Sample => "sample",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DclKind {
    /// Input or output with a semantic. `usage` is `None` for `vPos`/`vFace`.
    Semantic { usage: Option<Usage>, index: u8 },
    /// `dcl_2d s#`, `dcl_cube s#`, `dcl_volume s#`.
    Sampler(SamplerKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dcl {
    pub kind: DclKind,
    pub dst: Dst,
}

/// One statement of the shader body, in token order.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Dcl(Dcl),
    DefF { reg: Reg, value: [f32; 4] },
    DefI { reg: Reg, value: [i32; 4] },
    DefB { reg: Reg, value: bool },
    Inst(Inst),
}

/// A comment token other than the constant table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub data: Vec<u8>,
}

/// A parsed SM3 block.
#[derive(Clone, Debug, PartialEq)]
pub struct Shader {
    pub version: Version,
    pub ctab: Option<Ctab>,
    pub comments: Vec<Comment>,
    pub stmts: Vec<Stmt>,
}

impl Shader {
    /// Instructions (`Stmt::Inst`) only.
    pub fn insts(&self) -> impl Iterator<Item = &Inst> {
        self.stmts.iter().filter_map(|s| match s {
            Stmt::Inst(i) => Some(i),
            _ => None,
        })
    }
}
