//! Register analysis of the oracle (SR-D5): which registers a block uses, what the module has to
//! declare for them, and the names of the sampler variables.

use std::collections::{BTreeMap, BTreeSet};

use super::{IoReg, Options, OracleError, SamplerRes};
use crate::scan::Stage;
use crate::sm3::Shader;
use crate::sm3::ctab::RegisterSet;
use crate::sm3::ir::*;

/// Float constant registers of a vertex shader (vs_3_0, d3d9caps `MaxVertexShaderConst`).
pub const MAX_FLOAT_VS: u32 = 256;
/// Float constant registers of a pixel shader (ps_3_0).
pub const MAX_FLOAT_PS: u32 = 224;
/// Integer constant registers `i0..i15`.
pub const INT_REGS: u32 = 16;
/// Temporary registers `r0..r31`.
pub const TEMP_REGS: u32 = 32;
/// Bit set `b0..b15` fits in one `u32` (SR-D5).
pub const BOOL_REGS: u32 = 32;
/// Constant block of the oracle: `@group(0) @binding(0)`.
pub const CONST_GROUP: u32 = 0;
pub const CONST_BINDING: u32 = 0;
/// Sampler pairs: `@group(1)`, texture at `2 * s#`, sampler at `2 * s# + 1`.
pub const SAMPLER_GROUP: u32 = 1;

/// What a block needs, from one walk over its statements.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub stage: Stage,
    /// Temporaries `r0..r(temps-1)`.
    pub temps: u32,
    /// Registers in the `c` array (0 = none).
    pub float_regs: u32,
    /// Some source reads a constant through `a0`.
    pub rel_const: bool,
    pub uses_int: bool,
    pub uses_bool: bool,
    pub inputs: Vec<IoReg>,
    pub outputs: Vec<IoReg>,
    pub oc: BTreeSet<u32>,
    pub depth: bool,
    pub a0: bool,
    pub texkill: bool,
    pub samplers: Vec<SamplerRes>,
    pub defs_f: BTreeMap<u32, [f32; 4]>,
    pub defs_i: BTreeMap<u32, [i32; 4]>,
    pub defs_b: BTreeMap<u32, bool>,
    pub float_names: Vec<(String, u32, u32)>,
}

impl Analysis {
    pub fn input(&self, reg: u32) -> Option<&IoReg> {
        self.inputs
            .iter()
            .find(|i| i.reg == reg && i.usage.is_some())
    }
    pub fn vpos(&self) -> bool {
        self.inputs.iter().any(|i| i.usage.is_none() && i.reg == 0)
    }
    pub fn vface(&self) -> bool {
        self.inputs.iter().any(|i| i.usage.is_none() && i.reg == 1)
    }
    pub fn sampler(&self, reg: u32) -> Option<&SamplerRes> {
        self.samplers.iter().find(|s| s.reg == reg)
    }
}

/// WGSL keywords and reserved words, and the few builtin names a CTAB name could hit.
const RESERVED: &[&str] = &[
    "alias",
    "break",
    "case",
    "const",
    "const_assert",
    "continue",
    "continuing",
    "default",
    "diagnostic",
    "discard",
    "else",
    "enable",
    "false",
    "fn",
    "for",
    "if",
    "let",
    "loop",
    "override",
    "requires",
    "return",
    "struct",
    "switch",
    "true",
    "var",
    "while",
    "bool",
    "f16",
    "f32",
    "i32",
    "u32",
    "array",
    "atomic",
    "ptr",
    "sampler",
    "sampler_comparison",
    "vec2",
    "vec3",
    "vec4",
    "mat2x2",
    "mat3x3",
    "mat4x4",
    "texture",
    "bitcast",
    "abs",
    "sin",
    "cos",
    "exp",
    "log",
    "pow",
    "min",
    "max",
    "mix",
    "dot",
    "select",
    "all",
    "any",
    "clamp",
    "floor",
    "fract",
    "length",
    "normalize",
    "sqrt",
    "step",
    "inp",
    "ora",
    "in",
    "out",
    "r",
    "a0",
];

/// Prefixes of the names the oracle declares itself.
const OWN_PREFIXES: &[&str] = &["d3d_", "oracle_", "Oracle", "ora_"];

fn plain_ident(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && it.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with("__")
        && s != "_"
}

fn usable_name(s: &str, taken: &BTreeSet<String>) -> bool {
    // Names starting with `_` are the locals of the body: a texture must not hide one.
    plain_ident(s)
        && !s.starts_with('_')
        && !RESERVED.contains(&s)
        && !OWN_PREFIXES.iter().any(|p| s.starts_with(p))
        && !taken.contains(s)
        && !taken.contains(&format!("{s}_smp"))
        && !s.ends_with("_smp")
}

pub fn analyze(sh: &Shader, label: &str, _opts: &Options) -> Result<Analysis, OracleError> {
    let stage = sh.version.stage;
    let bad = |what: String| OracleError::Invalid {
        block: label.to_string(),
        what,
    };
    if sh.version.major != 3 || sh.version.minor != 0 {
        return Err(bad(format!("{} is not a 3_0 shader", sh.version.profile())));
    }
    let mut an = Analysis {
        stage,
        temps: 0,
        float_regs: 0,
        rel_const: false,
        uses_int: false,
        uses_bool: false,
        inputs: Vec::new(),
        outputs: Vec::new(),
        oc: BTreeSet::new(),
        depth: false,
        a0: false,
        texkill: false,
        samplers: Vec::new(),
        defs_f: BTreeMap::new(),
        defs_i: BTreeMap::new(),
        defs_b: BTreeMap::new(),
        float_names: Vec::new(),
    };
    // Declarations and definitions first: they are all at the top, but a block may repeat them.
    let mut sampler_kinds: BTreeMap<u32, SamplerKind> = BTreeMap::new();
    for st in &sh.stmts {
        match st {
            Stmt::Dcl(d) => match (d.kind, d.dst.reg.ty) {
                (DclKind::Sampler(k), RegType::Sampler) => {
                    sampler_kinds.insert(d.dst.reg.num, k);
                }
                (DclKind::Semantic { usage, index }, RegType::Input) => an.inputs.push(IoReg {
                    reg: d.dst.reg.num,
                    usage,
                    index,
                    mask: d.dst.mask,
                    centroid: d.dst.centroid(),
                    location: Some(d.dst.reg.num),
                }),
                (DclKind::Semantic { usage: None, .. }, RegType::MiscType) => {
                    an.inputs.push(IoReg {
                        reg: d.dst.reg.num,
                        usage: None,
                        index: 0,
                        mask: d.dst.mask,
                        centroid: false,
                        location: None,
                    })
                }
                (DclKind::Semantic { usage, index }, RegType::Output) if stage == Stage::Vs => {
                    an.outputs.push(IoReg {
                        reg: d.dst.reg.num,
                        usage,
                        index,
                        mask: d.dst.mask,
                        centroid: false,
                        location: Some(d.dst.reg.num),
                    })
                }
                _ => {
                    return Err(bad(format!(
                        "dcl of register {:?} is not supported",
                        d.dst.reg
                    )));
                }
            },
            Stmt::DefF { reg, value } if reg.ty == RegType::Const => {
                an.defs_f.insert(reg.num, *value);
            }
            Stmt::DefI { reg, value } if reg.ty == RegType::ConstInt => {
                an.defs_i.insert(reg.num, *value);
            }
            Stmt::DefB { reg, value } if reg.ty == RegType::ConstBool => {
                an.defs_b.insert(reg.num, *value);
            }
            Stmt::DefF { .. } | Stmt::DefI { .. } | Stmt::DefB { .. } => {
                return Err(bad("def of a register that is not c#, i# or b#".into()));
            }
            Stmt::Inst(_) => {}
        }
    }
    // The vertex shader's position output: location is not used for it.
    for o in &mut an.outputs {
        if o.usage == Some(Usage::Position) && o.index == 0 {
            o.location = None;
        }
    }

    let limit = if stage == Stage::Vs {
        MAX_FLOAT_VS
    } else {
        MAX_FLOAT_PS
    };
    let mut max_c: Option<u32> = None;
    let mut touch_src = |an: &mut Analysis, s: &Src| match s.reg.ty {
        RegType::Temp => an.temps = an.temps.max(s.reg.num + 1),
        RegType::Const => {
            if s.rel.is_some() {
                an.rel_const = true;
            } else if !an.defs_f.contains_key(&s.reg.num) {
                max_c = Some(max_c.map_or(s.reg.num, |m| m.max(s.reg.num)));
            }
        }
        RegType::ConstInt if !an.defs_i.contains_key(&s.reg.num) => an.uses_int = true,
        RegType::ConstBool if !an.defs_b.contains_key(&s.reg.num) => an.uses_bool = true,
        _ => {}
    };
    for i in sh.insts() {
        for s in &i.src {
            touch_src(&mut an, s);
        }
        if let Some(d) = &i.dst {
            match d.reg.ty {
                RegType::Temp => an.temps = an.temps.max(d.reg.num + 1),
                RegType::ColorOut => {
                    an.oc.insert(d.reg.num);
                }
                RegType::DepthOut => an.depth = true,
                RegType::Addr => an.a0 = true,
                _ => {}
            }
            if i.op == Opcode::TexKill {
                an.texkill = true;
            }
        }
    }
    if an.temps > TEMP_REGS {
        return Err(bad(format!(
            "{} temporaries, the limit is {TEMP_REGS}",
            an.temps
        )));
    }

    // CTAB: names of the float constants (for the harness) and the extent of the arrays that are
    // read through `a0`.
    let mut ctab_end = 0u32;
    if let Some(ct) = &sh.ctab {
        for c in &ct.constants {
            if c.set == RegisterSet::Float4 {
                an.float_names
                    .push((c.name.clone(), c.reg as u32, c.count as u32));
                ctab_end = ctab_end.max(c.reg as u32 + c.count as u32);
            }
        }
    }
    let mut n = max_c.map_or(0, |m| m + 1);
    if an.rel_const {
        n = n.max(ctab_end);
        if let Some(m) = an.defs_f.keys().max() {
            n = n.max(m + 1);
        }
    }
    if n > limit {
        return Err(bad(format!(
            "{n} float constant registers, {} allows {limit}",
            sh.version.profile()
        )));
    }
    an.float_regs = n;

    // Sampler variables: named like the CTAB constant, when that is a usable WGSL name.
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for (reg, kind) in &sampler_kinds {
        let ctab_name = sh
            .ctab
            .as_ref()
            .and_then(|ct| ct.find(RegisterSet::Sampler, *reg))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("s{reg}"));
        let var = if usable_name(&ctab_name, &taken) {
            ctab_name.clone()
        } else {
            format!(
                "tex{reg}_{}",
                ctab_name.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
            )
        };
        taken.insert(var.clone());
        taken.insert(format!("{var}_smp"));
        an.samplers.push(SamplerRes {
            reg: *reg,
            ctab_name,
            kind: *kind,
            sampler_var: format!("{var}_smp"),
            texture_var: var,
            group: SAMPLER_GROUP,
            texture_binding: 2 * reg,
            sampler_binding: 2 * reg + 1,
        });
    }
    Ok(an)
}
