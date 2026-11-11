//! UI v2 skin package: the `C5UI2PKG` container, its sections and the machine-readable UI profile.
//!
//! A skin is one deterministic file: layout, baked styles, tokens, bindings, texts, scripts, fonts,
//! pictures and references to game assets. `Package::read` parses and checks it (it never panics on
//! any bytes and compiles for wasm32); `PackageBuilder` (feature `write`) is the only writer; the
//! `profile` module holds the schema types the package shares with the converter and the runtime,
//! and, behind feature `profile`, the profile tables and parsers.

pub mod container;
mod error;
pub mod ids;
mod package;
pub mod profile;
pub mod sections;
#[cfg(feature = "write")]
mod write;

pub use container::{FORMAT_MAJOR, FORMAT_MINOR, PackageHash, Tag};
pub use error::PackageError;
pub use ids::{
    ARefId, ExprId, ImgId, NodeRef, StrId, StyleId, StyleSetId, TemplateId, TokenId, TplId,
};
pub use package::Package;
pub use sections::manifest::{FaceFamily, FontRoles, Kind, Manifest, Tool};
#[cfg(feature = "write")]
pub use write::PackageBuilder;
