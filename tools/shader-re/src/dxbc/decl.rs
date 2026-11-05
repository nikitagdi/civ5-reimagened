//! The program chunk (`SHDR`/`SHEX`): version, and the `dcl_*` declarations (SR-D2). Instruction bodies are not decoded: the walk only needs each instruction's length (the
//! token stream is self-delimiting), and the text of the instructions comes from `D3DDisassemble`
//! (`asm_text`).
//!
//! Token layout (Microsoft `d3d10tokenizedprogramformat.hpp` / `d3d11tokenizedprogramformat.hpp`):
//! - version token: minor 0-3, major 4-7, program type 16-31; then the length in dwords;
//! - instruction token: opcode 0-10, opcode-specific control 11-23, length 24-30, extended bit 31;
//!   `customdata` (opcode 53) carries its length in the next dword;
//! - operand token: components 0-1, selection mode 2-3, mask / swizzle 4-11, type 12-19, index
//!   dimension 20-21, index representations 22-24 / 25-27 / 28-30, extended bit 31.

use super::DxbcError;
use super::chunks::u32_at;
use crate::scan::Stage;

// Opcode numbers of the declarations we read (`D3D10_SB_OPCODE_TYPE`, `D3D11_SB_OPCODE_TYPE`).
pub const OP_CUSTOMDATA: u32 = 53;
pub const OP_DCL_RESOURCE: u32 = 88;
pub const OP_DCL_CONSTANT_BUFFER: u32 = 89;
pub const OP_DCL_SAMPLER: u32 = 90;
pub const OP_DCL_INPUT: u32 = 95;
pub const OP_DCL_INPUT_SGV: u32 = 96;
pub const OP_DCL_INPUT_SIV: u32 = 97;
pub const OP_DCL_INPUT_PS: u32 = 98;
pub const OP_DCL_INPUT_PS_SGV: u32 = 99;
pub const OP_DCL_INPUT_PS_SIV: u32 = 100;
pub const OP_DCL_OUTPUT: u32 = 101;
pub const OP_DCL_OUTPUT_SGV: u32 = 102;
pub const OP_DCL_OUTPUT_SIV: u32 = 103;
pub const OP_DCL_TEMPS: u32 = 104;
pub const OP_DCL_INDEXABLE_TEMP: u32 = 105;
pub const OP_DCL_GLOBAL_FLAGS: u32 = 106;
/// `hs_decls`, `hs_control_point_phase`, `hs_fork_phase`, `hs_join_phase` markers.
const OP_PHASE_FIRST: u32 = 113;
const OP_PHASE_LAST: u32 = 116;
/// `dcl_input_control_point_count` .. `dcl_resource_structured` of SM5 (tessellation, UAV, raw and
/// structured resources, interfaces).
const OP_SM5_DCL_FIRST: u32 = 143;
const OP_SM5_DCL_LAST: u32 = 162;
/// Sample, load and gather instructions: the texture fetches (`sample` .. `sample_b`, `ld`,
/// `ld_ms`, `lod`, `gather4`, `gather4_c`, `gather4_po`, `gather4_po_c`).
const FETCH_OPS: [u32; 14] = [45, 46, 69, 70, 71, 72, 73, 74, 108, 109, 126, 127, 128, 110];

const OPERAND_TYPE_SHIFT: u32 = 12;
const OPERAND_TYPE_MASK: u32 = 0xFF;
const OPERAND_DIM_SHIFT: u32 = 20;
const OPERAND_REP_SHIFT: u32 = 22;
const OPERAND_EXTENDED: u32 = 1 << 31;
/// `D3D10_SB_OPERAND_TYPE`.
pub const OPERAND_INPUT: u32 = 1;
pub const OPERAND_OUTPUT: u32 = 2;
/// `D3D10_SB_OPERAND_INDEX_IMMEDIATE32`.
const INDEX_IMM32: u32 = 0;

/// Resource dimension of `dcl_resource` (`D3D10_SB_RESOURCE_DIMENSION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResDim {
    Unknown,
    Buffer,
    Tex1d,
    Tex2d,
    Tex2dMs,
    Tex3d,
    Cube,
    Tex1dArray,
    Tex2dArray,
    Tex2dMsArray,
    CubeArray,
    RawBuffer,
    StructuredBuffer,
}

impl ResDim {
    pub fn from_code(c: u32) -> ResDim {
        match c {
            1 => ResDim::Buffer,
            2 => ResDim::Tex1d,
            3 => ResDim::Tex2d,
            4 => ResDim::Tex2dMs,
            5 => ResDim::Tex3d,
            6 => ResDim::Cube,
            7 => ResDim::Tex1dArray,
            8 => ResDim::Tex2dArray,
            9 => ResDim::Tex2dMsArray,
            10 => ResDim::CubeArray,
            11 => ResDim::RawBuffer,
            12 => ResDim::StructuredBuffer,
            _ => ResDim::Unknown,
        }
    }

    /// As in `dcl_resource_<name>` of the listing.
    pub fn name(self) -> &'static str {
        match self {
            ResDim::Unknown => "unknown",
            ResDim::Buffer => "buffer",
            ResDim::Tex1d => "texture1d",
            ResDim::Tex2d => "texture2d",
            ResDim::Tex2dMs => "texture2dms",
            ResDim::Tex3d => "texture3d",
            ResDim::Cube => "texturecube",
            ResDim::Tex1dArray => "texture1darray",
            ResDim::Tex2dArray => "texture2darray",
            ResDim::Tex2dMsArray => "texture2dmsarray",
            ResDim::CubeArray => "texturecubearray",
            ResDim::RawBuffer => "raw_buffer",
            ResDim::StructuredBuffer => "structured_buffer",
        }
    }

    /// The SM3 sampler kind (`2d`, `cube`, `volume`) this resource corresponds to, if it has one.
    pub fn sm3_kind(self) -> Option<&'static str> {
        match self {
            ResDim::Tex1d => Some("1d"),
            ResDim::Tex2d => Some("2d"),
            ResDim::Cube => Some("cube"),
            ResDim::Tex3d => Some("volume"),
            _ => None,
        }
    }
}

/// `dcl_constantbuffer cb<reg>[<size>]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CbDecl {
    pub reg: u32,
    /// Size in float4.
    pub size: u32,
    /// `dynamicIndexed` (bit 11 of the control field), else `immediateIndexed`.
    pub dynamic: bool,
}

/// `dcl_resource_<dim> (<return types>) t<reg>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResDecl {
    pub reg: u32,
    pub dim: ResDim,
    /// Return type per component (`D3D10_SB_RESOURCE_RETURN_TYPE`: 1 unorm, 2 snorm, 3 sint,
    /// 4 uint, 5 float, 6 mixed).
    pub ret: [u8; 4],
}

/// `dcl_sampler s<reg>, mode_<mode>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerDecl {
    pub reg: u32,
    /// 0 default, 1 comparison, 2 mono.
    pub mode: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoKind {
    Input,
    InputSgv,
    InputSiv,
    InputPs,
    InputPsSgv,
    InputPsSiv,
    Output,
    OutputSgv,
    OutputSiv,
}

impl IoKind {
    pub fn is_input(self) -> bool {
        !matches!(self, IoKind::Output | IoKind::OutputSgv | IoKind::OutputSiv)
    }
}

/// `dcl_input*` / `dcl_output*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoDecl {
    pub kind: IoKind,
    /// `D3D10_SB_OPERAND_TYPE`: 1 input, 2 output, 12 `oDepth`, ...
    pub operand: u32,
    /// Register of an indexed operand; `None` for `oDepth` and the like.
    pub reg: Option<u32>,
    /// Components, bit 0 = x (0 for a scalar operand).
    pub mask: u8,
    /// `D3D10_SB_NAME` of the `_sgv`/`_siv` forms.
    pub sysval: Option<u32>,
    /// Interpolation of `dcl_input_ps*` (control field).
    pub interp: Option<u8>,
}

/// What the declarations of one program say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decls {
    pub cbuffers: Vec<CbDecl>,
    pub resources: Vec<ResDecl>,
    pub samplers: Vec<SamplerDecl>,
    pub inputs: Vec<IoDecl>,
    pub outputs: Vec<IoDecl>,
    /// `dcl_temps`.
    pub temps: u32,
    /// `dcl_globalFlags` control bits.
    pub global_flags: u32,
    /// Declarations the reader skips (tessellation, UAV, interfaces ...), by opcode.
    pub other_decls: Vec<u32>,
    /// `customdata` blocks (immediate constant buffers, comments).
    pub custom_data: usize,
    /// Instructions: everything that is not a declaration.
    pub instructions: usize,
    /// Texture fetch instructions.
    pub fetches: usize,
}

/// Program type, version and declarations of the `SHDR`/`SHEX` chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub stage: Stage,
    pub major: u8,
    pub minor: u8,
    /// Length of the program in dwords, from the second token.
    pub dwords: u32,
    pub decls: Decls,
}

impl Program {
    /// `ps_5_0`, `vs_4_1`, ...
    pub fn profile(&self) -> String {
        let p = match self.stage {
            Stage::Ps => "ps",
            Stage::Vs => "vs",
            Stage::Gs => "gs",
            Stage::Hs => "hs",
            Stage::Ds => "ds",
            Stage::Cs => "cs",
        };
        format!("{p}_{}_{}", self.major, self.minor)
    }
}

fn stage_of(program_type: u32) -> Option<Stage> {
    Some(match program_type {
        0 => Stage::Ps,
        1 => Stage::Vs,
        2 => Stage::Gs,
        3 => Stage::Hs,
        4 => Stage::Ds,
        5 => Stage::Cs,
        _ => return None,
    })
}

fn is_decl(op: u32) -> bool {
    (OP_DCL_RESOURCE..=OP_DCL_GLOBAL_FLAGS).contains(&op)
        || (OP_PHASE_FIRST..=OP_PHASE_LAST).contains(&op)
        || (OP_SM5_DCL_FIRST..=OP_SM5_DCL_LAST).contains(&op)
}

/// An operand as the declarations use it: type, mask and the immediate indices.
struct Operand {
    ty: u32,
    mask: u8,
    indices: Vec<u32>,
    /// Dwords the operand took (token, extended tokens, indices).
    len: usize,
}

fn read_operand(t: &[u32], at: usize, opcode: u32) -> Result<Operand, DxbcError> {
    let bad = |why: &str| DxbcError::Program(format!("dcl opcode {opcode}: {why}"));
    let tok = *t.get(at).ok_or_else(|| bad("no operand token"))?;
    let ty = (tok >> OPERAND_TYPE_SHIFT) & OPERAND_TYPE_MASK;
    let dim = ((tok >> OPERAND_DIM_SHIFT) & 3) as usize;
    let mut pos = at + 1;
    // Extended operand tokens (modifiers): each says whether another follows.
    let mut ext = tok & OPERAND_EXTENDED != 0;
    while ext {
        let e = *t.get(pos).ok_or_else(|| bad("extended token is missing"))?;
        ext = e & OPERAND_EXTENDED != 0;
        pos += 1;
    }
    let mut indices = Vec::with_capacity(dim);
    for d in 0..dim {
        let rep = (tok >> (OPERAND_REP_SHIFT + 3 * d as u32)) & 7;
        if rep != INDEX_IMM32 {
            return Err(bad("operand index is not an immediate"));
        }
        indices.push(*t.get(pos).ok_or_else(|| bad("index is missing"))?);
        pos += 1;
    }
    // Component selection: 4-component operand in mask mode carries the mask in bits 4-7.
    let num_comp = tok & 3;
    let sel_mode = (tok >> 2) & 3;
    let mask = if num_comp == 2 && sel_mode == 0 {
        ((tok >> 4) & 0xF) as u8
    } else {
        0
    };
    Ok(Operand {
        ty,
        mask,
        indices,
        len: pos - at,
    })
}

/// Walks the program chunk: version, then every instruction by its length.
pub fn parse_program(payload: &[u8]) -> Result<Program, DxbcError> {
    if !payload.len().is_multiple_of(4) || payload.len() < 8 {
        return Err(DxbcError::Program(format!(
            "program chunk of {} bytes",
            payload.len()
        )));
    }
    let t: Vec<u32> = (0..payload.len() / 4)
        .map(|i| u32_at(payload, i * 4).expect("in range"))
        .collect();
    let version = t[0];
    let (minor, major, ty) = (version & 0xF, (version >> 4) & 0xF, version >> 16);
    let stage = stage_of(ty).ok_or_else(|| DxbcError::Program(format!("program type {ty}")))?;
    if t[1] as usize != t.len() {
        return Err(DxbcError::Program(format!(
            "program says {} dwords, the chunk holds {}",
            t[1],
            t.len()
        )));
    }
    let mut d = Decls::default();
    let mut pos = 2;
    while pos < t.len() {
        let tok = t[pos];
        let op = tok & 0x7FF;
        let control = (tok >> 11) & 0x1FFF;
        let len = if op == OP_CUSTOMDATA {
            *t.get(pos + 1)
                .ok_or_else(|| DxbcError::Program("customdata without length".into()))?
                as usize
        } else {
            ((tok >> 24) & 0x7F) as usize
        };
        if len == 0 || pos + len > t.len() {
            return Err(DxbcError::Program(format!(
                "instruction {op} at dword {pos} has length {len}"
            )));
        }
        let body = &t[pos..pos + len];
        match op {
            OP_CUSTOMDATA => d.custom_data += 1,
            OP_DCL_RESOURCE => {
                let o = read_operand(body, 1, op)?;
                let ret = *body
                    .get(1 + o.len)
                    .ok_or_else(|| DxbcError::Program("dcl_resource without return type".into()))?;
                d.resources.push(ResDecl {
                    reg: *o.indices.first().unwrap_or(&0),
                    dim: ResDim::from_code(control & 0x1F),
                    ret: [
                        (ret & 0xF) as u8,
                        ((ret >> 4) & 0xF) as u8,
                        ((ret >> 8) & 0xF) as u8,
                        ((ret >> 12) & 0xF) as u8,
                    ],
                });
            }
            OP_DCL_CONSTANT_BUFFER => {
                let o = read_operand(body, 1, op)?;
                if o.indices.len() != 2 {
                    return Err(DxbcError::Program(format!(
                        "dcl_constantbuffer with {} indices",
                        o.indices.len()
                    )));
                }
                d.cbuffers.push(CbDecl {
                    reg: o.indices[0],
                    size: o.indices[1],
                    dynamic: control & 1 != 0,
                });
            }
            OP_DCL_SAMPLER => {
                let o = read_operand(body, 1, op)?;
                d.samplers.push(SamplerDecl {
                    reg: *o.indices.first().unwrap_or(&0),
                    mode: (control & 0xF) as u8,
                });
            }
            OP_DCL_INPUT | OP_DCL_INPUT_SGV | OP_DCL_INPUT_SIV | OP_DCL_INPUT_PS
            | OP_DCL_INPUT_PS_SGV | OP_DCL_INPUT_PS_SIV | OP_DCL_OUTPUT | OP_DCL_OUTPUT_SGV
            | OP_DCL_OUTPUT_SIV => {
                let o = read_operand(body, 1, op)?;
                let kind = match op {
                    OP_DCL_INPUT => IoKind::Input,
                    OP_DCL_INPUT_SGV => IoKind::InputSgv,
                    OP_DCL_INPUT_SIV => IoKind::InputSiv,
                    OP_DCL_INPUT_PS => IoKind::InputPs,
                    OP_DCL_INPUT_PS_SGV => IoKind::InputPsSgv,
                    OP_DCL_INPUT_PS_SIV => IoKind::InputPsSiv,
                    OP_DCL_OUTPUT => IoKind::Output,
                    OP_DCL_OUTPUT_SGV => IoKind::OutputSgv,
                    _ => IoKind::OutputSiv,
                };
                let sysval = matches!(
                    op,
                    OP_DCL_INPUT_SGV
                        | OP_DCL_INPUT_SIV
                        | OP_DCL_INPUT_PS_SGV
                        | OP_DCL_INPUT_PS_SIV
                        | OP_DCL_OUTPUT_SGV
                        | OP_DCL_OUTPUT_SIV
                )
                .then(|| body.get(1 + o.len).copied())
                .flatten();
                let io = IoDecl {
                    kind,
                    operand: o.ty,
                    reg: o.indices.first().copied(),
                    mask: o.mask,
                    sysval,
                    interp: matches!(
                        op,
                        OP_DCL_INPUT_PS | OP_DCL_INPUT_PS_SGV | OP_DCL_INPUT_PS_SIV
                    )
                    .then_some((control & 0xF) as u8),
                };
                if kind.is_input() {
                    d.inputs.push(io);
                } else {
                    d.outputs.push(io);
                }
            }
            OP_DCL_TEMPS => d.temps = *body.get(1).unwrap_or(&0),
            OP_DCL_GLOBAL_FLAGS => d.global_flags = control,
            op if is_decl(op) => d.other_decls.push(op),
            op => {
                d.instructions += 1;
                if FETCH_OPS.contains(&op) {
                    d.fetches += 1;
                }
            }
        }
        pos += len;
    }
    Ok(Program {
        stage,
        major: major as u8,
        minor: minor as u8,
        dwords: t[1],
        decls: d,
    })
}

#[cfg(test)]
pub(crate) mod synth {
    //! Synthetic programs for tests: the tests build their own tokens, nothing comes from the game.
    use super::*;

    /// Instruction token with `len` dwords in total.
    pub fn op(opcode: u32, control: u32, len: u32) -> u32 {
        opcode | (control << 11) | (len << 24)
    }

    /// Operand token: 4-component mask mode, type, index dimension, immediate indices.
    pub fn operand(ty: u32, dim: u32, mask: u32) -> u32 {
        2 | (mask << 4) | (ty << OPERAND_TYPE_SHIFT) | (dim << OPERAND_DIM_SHIFT)
    }

    pub fn program(program_type: u32, major: u32, minor: u32, body: &[u32]) -> Vec<u8> {
        let mut t = vec![minor | (major << 4) | (program_type << 16), 0];
        t.extend_from_slice(body);
        t[1] = t.len() as u32;
        t.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn dcl_cb(reg: u32, size: u32, dynamic: bool) -> Vec<u32> {
        vec![
            op(OP_DCL_CONSTANT_BUFFER, u32::from(dynamic), 4),
            operand(8, 2, 0),
            reg,
            size,
        ]
    }

    pub fn dcl_resource(dim: u32, reg: u32) -> Vec<u32> {
        // Return type float in all four components.
        vec![op(OP_DCL_RESOURCE, dim, 4), operand(7, 1, 0), reg, 0x5555]
    }

    pub fn dcl_sampler(reg: u32, mode: u32) -> Vec<u32> {
        vec![op(OP_DCL_SAMPLER, mode, 3), operand(6, 1, 0), reg]
    }

    pub fn dcl_input_ps(reg: u32, mask: u32) -> Vec<u32> {
        vec![
            op(OP_DCL_INPUT_PS, 2, 3),
            operand(OPERAND_INPUT, 1, mask),
            reg,
        ]
    }

    pub fn dcl_output(reg: u32, mask: u32) -> Vec<u32> {
        vec![
            op(OP_DCL_OUTPUT, 0, 3),
            operand(OPERAND_OUTPUT, 1, mask),
            reg,
        ]
    }

    /// `mov` with two operands of one dword each (a stand-in for any instruction).
    pub fn mov() -> Vec<u32> {
        vec![op(54, 0, 3), 0x0010_F012, 0x0010_F012]
    }

    /// `sample` with five dwords.
    pub fn sample() -> Vec<u32> {
        vec![op(69, 0, 5), 0, 0, 0, 0]
    }
}

#[cfg(test)]
mod tests {
    use super::synth::*;
    use super::*;

    #[test]
    fn declarations_are_read() {
        let mut body = Vec::new();
        body.extend([op(OP_DCL_GLOBAL_FLAGS, 1, 1)]);
        body.extend(dcl_cb(1, 12, false));
        body.extend(dcl_cb(0, 4, true));
        body.extend(dcl_sampler(0, 0));
        body.extend(dcl_sampler(1, 1));
        body.extend(dcl_resource(3, 0));
        body.extend(dcl_resource(6, 1));
        body.extend(dcl_input_ps(1, 15));
        body.extend(dcl_output(0, 15));
        // dcl_output_siv o0.xyzw, position: operand, index, name.
        body.extend([
            op(OP_DCL_OUTPUT_SIV, 0, 4),
            operand(OPERAND_OUTPUT, 1, 15),
            1,
            1,
        ]);
        body.extend([op(OP_DCL_TEMPS, 0, 2), 5]);
        // customdata of 6 dwords (class in the control bits, length in the second dword).
        body.extend([op(OP_CUSTOMDATA, 3, 0), 6, 1, 2, 3, 4]);
        body.extend(mov());
        body.extend(sample());
        body.extend([op(62, 0, 1)]); // ret
        let p = parse_program(&program(0, 5, 0, &body)).unwrap();
        assert_eq!(p.profile(), "ps_5_0");
        let d = &p.decls;
        assert_eq!(d.global_flags, 1);
        assert_eq!(
            d.cbuffers,
            [
                CbDecl {
                    reg: 1,
                    size: 12,
                    dynamic: false
                },
                CbDecl {
                    reg: 0,
                    size: 4,
                    dynamic: true
                }
            ]
        );
        assert_eq!(
            d.samplers,
            [
                SamplerDecl { reg: 0, mode: 0 },
                SamplerDecl { reg: 1, mode: 1 }
            ]
        );
        assert_eq!(d.resources.len(), 2);
        assert_eq!(d.resources[0].dim, ResDim::Tex2d);
        assert_eq!(d.resources[1].dim, ResDim::Cube);
        assert_eq!(d.resources[1].reg, 1);
        assert_eq!(d.resources[0].ret, [5, 5, 5, 5]);
        assert_eq!(d.inputs.len(), 1);
        assert_eq!(d.inputs[0].kind, IoKind::InputPs);
        assert_eq!(d.inputs[0].reg, Some(1));
        assert_eq!(d.inputs[0].mask, 15);
        assert_eq!(d.inputs[0].interp, Some(2));
        assert_eq!(d.outputs.len(), 2);
        assert_eq!(d.outputs[1].sysval, Some(1));
        assert_eq!(d.temps, 5);
        assert_eq!(d.custom_data, 1);
        assert_eq!(d.instructions, 3);
        assert_eq!(d.fetches, 1);
    }

    #[test]
    fn stages_and_versions() {
        let p = parse_program(&program(1, 4, 1, &[op(62, 0, 1)])).unwrap();
        assert_eq!((p.stage, p.profile().as_str()), (Stage::Vs, "vs_4_1"));
        let p = parse_program(&program(3, 5, 0, &[op(62, 0, 1)])).unwrap();
        assert_eq!(p.profile(), "hs_5_0");
        assert!(parse_program(&program(9, 5, 0, &[op(62, 0, 1)])).is_err());
    }

    #[test]
    fn broken_streams_are_errors() {
        let ok = program(0, 5, 0, &[op(54, 0, 3), 1, 2]);
        assert!(parse_program(&ok).is_ok());
        // Instruction longer than the chunk.
        assert!(parse_program(&program(0, 5, 0, &[op(54, 0, 9), 1, 2])).is_err());
        // Zero length.
        assert!(parse_program(&program(0, 5, 0, &[op(54, 0, 0)])).is_err());
        // Declared length differs from the chunk.
        let mut bad = ok.clone();
        bad[4] = 99;
        assert!(parse_program(&bad).is_err());
        // Not a whole number of dwords, or too short.
        assert!(parse_program(&ok[..ok.len() - 1]).is_err());
        assert!(parse_program(&[]).is_err());
        // Constant buffer with one index.
        let one = [op(OP_DCL_CONSTANT_BUFFER, 0, 3), operand(8, 1, 0), 0];
        assert!(parse_program(&program(0, 5, 0, &one)).is_err());
    }
}
