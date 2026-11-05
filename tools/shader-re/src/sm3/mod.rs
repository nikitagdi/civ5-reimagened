//! SM3 (D3D9 `vs_3_0`/`ps_3_0`) block parsing and disassembly.
//!
//! `token` reads the token stream into the `ir` types, `ctab` the constant table, `disasm`
//! prints the text of `D3DDisassemble`.

pub mod ctab;
pub mod disasm;
pub mod ir;
pub mod token;

pub use ctab::{Ctab, CtabConstant};
pub use disasm::disassemble;
pub use ir::Shader;
pub use token::{Sm3Error, parse};
