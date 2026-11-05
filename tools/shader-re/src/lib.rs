//! `shader-re`: shaders of the original game, extracted from the pinned binaries of your own install, and checks
//! for our hand-written WGSL. Everything derived from the binaries stays in the cache folder (`--cache`,
//! `CIV5_SHADER_RE_CACHE`, default `cache/shader-re/` next to the repository), never in git (SR-D6).

pub mod bins;
pub mod catalog;
pub mod check;
pub mod dxbc;
pub mod elf;
pub mod harness;
pub mod names;
pub mod oracle;
pub mod pe;
pub mod scan;
pub mod sha1;
pub mod sha256;
pub mod sm3;
pub mod status;
pub mod validate;
