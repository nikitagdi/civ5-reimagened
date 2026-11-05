//! Granny 3D (.gr2) reader: container, sections (raw and Oodle1), pointer fixups and the
//! self-describing type tree, exposed as a zero-copy object model.
//!
//! Supports 32-bit little-endian files of format revision 7 (what Civ5 ships). Stripped type
//! trees, where member names are replaced by `!crc32(name)`, are resolved through the built-in
//! dictionary in `data/member-names.txt`.
//!
//! ```no_run
//! # fn main() -> Result<(), granny::GrannyError> {
//! let bytes = std::fs::read("model.gr2").unwrap();
//! let file = granny::GrannyFile::parse(&bytes)?;
//! let root = file.root()?;
//! for bone in root.references("Skeletons")?.iter()? {
//!     let Some(skeleton) = bone? else { continue };
//!     println!("{} bones", skeleton.array("Bones")?.len());
//! }
//! # Ok(())
//! # }
//! ```

pub mod animation;
pub mod bind;
mod bytes;
pub mod check;
mod crc32;
pub mod curve;
mod error;
pub mod extended;
mod file;
mod header;
pub mod material;
pub mod mesh;
mod names;
mod object;
pub mod oodle1;
mod scene;
pub mod skeleton;
mod types;
mod walk;

#[cfg(test)]
mod curve_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod typed_tests;

pub use bytes::latin1;
pub use crc32::crc32;
pub use error::GrannyError;
pub use file::{GrannyFile, Ptr};
pub use header::{COMPRESSION_NONE, COMPRESSION_OODLE1, Header, MAGIC_LE32, SectionHeader};
pub use names::{
    fgx_name, known_fgx_names, known_member_names, member_name, name_hash, name_hash_bytes,
    name_hash_prefix,
};
pub use object::{Array, ArrayData, FixedArray, Object, Ref, RefArray, Str, Transform, Value};
pub use scene::{Model, Scene};
pub use types::{Member, MemberKind, NameSource, TypeDef};
pub use walk::WalkStats;
