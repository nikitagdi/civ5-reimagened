//! The oracle: SM3 to WGSL, one instruction at a time (SR-D3..D5).
//!
//! The oracle is not part of the product. It exists so that hand-written WGSL can be checked
//! against the original's semantics (`harness`, AC-R9.2). Its input is the parsed token stream
//! ([`crate::sm3::Shader`]), not text. Everything the table of 4.6 does not list is an error that
//! names the block and the instruction; there is no approximate translation.
//!
//! The text has this shape (names in `{}` come from the block):
//!
//! ```text
//! // oracle: <entry> sha1 <sha1> -- not for the product
//! <d3d_* helpers>                       // the semantics of sm3.wgsl (see HELPERS)
//! struct OracleConsts { c: array<vec4<f32>, N>, i: .., b: u32 }   // SR-D5, group 0 binding 0
//! @group(1) @binding(2 * s)     var <ctab name>: texture_2d<f32>; // one pair per sampler
//! @group(1) @binding(2 * s + 1) var <ctab name>_smp: sampler;
//! struct OracleIn / OracleOut          // the body interface: one vec4 per register
//! fn oracle_body(inp: OracleIn) -> OracleOut
//! @fragment fn oracle_fs / @vertex fn oracle_vs   // thin entry point around the body
//! ```
//!
//! Branches are flattened (SR-D4, `flatten`): both sides of an `if` are computed and the writes
//! go through an execution mask; `rep` stays a `for` loop with a uniform count.

pub mod emit;
pub mod flatten;
pub mod regs;

use std::collections::BTreeMap;

use crate::scan::Stage;
use crate::sm3::Shader;
use crate::sm3::ir::{SamplerKind, Usage};

/// The `d3d_*` functions: the exact semantics of SM3 where WGSL differs (4.6, "traps").
///
/// These are the same text as `render/src/shaders/common/sm3.wgsl` (render-core
/// 4.7). That file is not in the tree yet (R0.14), so this is the single copy until it lands;
/// `tests/oracle_semantics.rs` pins the behaviour, and the day `sm3.wgsl` exists a test must
/// compare the two texts.
pub const HELPERS: &str = r#"
// D3D9 semantics that WGSL builtins do not give.
fn d3d_inf(sign_of: f32) -> f32 {
    // +inf, or -inf when `sign_of` has its sign bit set (so that rcp(-0) = -inf).
    return bitcast<f32>(0x7f800000u | (bitcast<u32>(sign_of) & 0x80000000u));
}
fn d3d_rcp(x: f32) -> f32 {
    return select(1.0 / x, d3d_inf(x), x == 0.0);
}
fn d3d_rsq(x: f32) -> f32 {
    return select(inverseSqrt(abs(x)), d3d_inf(0.0), x == 0.0);
}
fn d3d_log(x: f32) -> f32 {
    return select(log2(abs(x)), -d3d_inf(0.0), x == 0.0);
}
fn d3d_pow(x: f32, y: f32) -> f32 {
    // D3D9: 0 times anything is 0, so pow(x, 0) = exp2(0) = 1 for every x, also pow(0, 0).
    return exp2(select(y * d3d_log(x), 0.0, y == 0.0));
}
fn d3d_nrm(v: vec4<f32>) -> vec4<f32> {
    // D3D9: 0 times anything is 0, so nrm(0) is the zero vector, not 0 * inf = NaN.
    return select(v * d3d_rsq(dot(v.xyz, v.xyz)), vec4<f32>(0.0), v == vec4<f32>(0.0));
}
fn d3d_cmp(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> vec4<f32> {
    return select(c, b, a >= vec4<f32>(0.0));
}
"#;

/// What an oracle needs to know about a sampler whose original format differs from ours
/// (`maps/formats.toml`, 4.8): the oracle sees what D3D9 would have seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chan {
    R,
    G,
    B,
    A,
    Zero,
    One,
}

impl Chan {
    pub fn parse(c: char) -> Option<Chan> {
        Some(match c {
            'r' => Chan::R,
            'g' => Chan::G,
            'b' => Chan::B,
            'a' => Chan::A,
            '0' => Chan::Zero,
            '1' => Chan::One,
            _ => return None,
        })
    }
}

/// Quantisation of a sampled value before the channel selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quantize {
    #[default]
    None,
    /// 16 bit UNORM: `round(x * 65535) / 65535` (the LEAN textures, stored as `Rgba16Float`).
    Unorm16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatView {
    /// Where each of x, y, z, w of the D3D9 value comes from.
    pub chans: [Chan; 4],
    pub quantize: Quantize,
}

impl FormatView {
    pub const IDENTITY: FormatView = FormatView {
        chans: [Chan::R, Chan::G, Chan::B, Chan::A],
        quantize: Quantize::None,
    };
}

/// Options of [`translate`].
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// By CTAB sampler name: formats whose D3D9 view differs from ours.
    pub formats: BTreeMap<String, FormatView>,
}

/// A sampler register: the texture and sampler variables of the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplerRes {
    pub reg: u32,
    /// CTAB name (`s#` when the table has none).
    pub ctab_name: String,
    pub kind: SamplerKind,
    /// WGSL variable names: the texture is named like the CTAB constant.
    pub texture_var: String,
    pub sampler_var: String,
    pub group: u32,
    pub texture_binding: u32,
    pub sampler_binding: u32,
}

/// A declared input or output register.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoReg {
    pub reg: u32,
    /// `None` for `vPos` and `vFace`.
    pub usage: Option<Usage>,
    pub index: u8,
    /// Declared components, bit 0 = x.
    pub mask: u8,
    pub centroid: bool,
    /// `@location` in the entry point.
    pub location: Option<u32>,
}

/// The constant block (SR-D5): `array<vec4<f32>, float_regs>` and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstBlock {
    pub group: u32,
    pub binding: u32,
    /// `c#` registers in the array (0 = no float array).
    pub float_regs: u32,
    /// `i#` registers in the block (0 or 16).
    pub int_regs: u32,
    /// The block has the `b` mask.
    pub bool_mask: bool,
}

impl ConstBlock {
    /// Byte offset of `c` (always 0), `i` and `b` in the uniform buffer (std140-compatible
    /// arrays of `vec4`, then a `u32`).
    pub fn i_offset(&self) -> usize {
        self.float_regs as usize * 16
    }
    pub fn b_offset(&self) -> usize {
        self.i_offset() + self.int_regs as usize * 16
    }
    /// Size of the buffer: the struct size, rounded up to 16 bytes.
    pub fn size(&self) -> usize {
        let end = self.b_offset() + if self.bool_mask { 4 } else { 0 };
        end.next_multiple_of(16).max(16)
    }
}

/// The result of [`translate`].
#[derive(Clone, Debug)]
pub struct Oracle {
    /// `Package::Entry:STAGE` or whatever the caller named the block.
    pub name: String,
    pub sha1: String,
    pub stage: Stage,
    pub wgsl: String,
    /// `oracle_fs` or `oracle_vs`.
    pub entry_point: String,
    /// `oracle_body`: takes `OracleIn`, returns `OracleOut` (one vec4 per register).
    pub body: String,
    pub consts: Option<ConstBlock>,
    pub samplers: Vec<SamplerRes>,
    /// Inputs (`v#`), including `vPos` and `vFace` when declared.
    pub inputs: Vec<IoReg>,
    /// Outputs: `o#` of a vertex shader (declared), `oC#` of a pixel shader.
    pub outputs: Vec<IoReg>,
    /// The pixel shader writes `oDepth`.
    pub depth_out: bool,
    /// The pixel shader has `texkill`.
    pub texkill: bool,
    /// CTAB float constants as `(name, register, count)`.
    pub float_names: Vec<(String, u32, u32)>,
    /// The block's constant table: the harness fills hand-written constants from it by name.
    pub ctab: Option<crate::sm3::Ctab>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OracleError {
    /// An instruction or operand the table of 4.6 does not cover.
    #[error("{block}: instruction {index} `{text}`: not supported: {what}")]
    Unsupported {
        block: String,
        index: usize,
        text: String,
        what: String,
    },
    /// The block itself is not usable (undeclared register, too many constants ...).
    #[error("{block}: {what}")]
    Invalid { block: String, what: String },
}

/// Translates one parsed block. `name` and `sha1` go into the header comment and the errors.
pub fn translate(
    sh: &Shader,
    name: &str,
    sha1: &str,
    opts: &Options,
) -> Result<Oracle, OracleError> {
    let label = format!("{name} ({})", &sha1[..sha1.len().min(12)]);
    let an = regs::analyze(sh, &label, opts)?;
    emit::emit(sh, name, sha1, &label, &an, opts)
}
