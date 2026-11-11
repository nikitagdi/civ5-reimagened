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
#[cfg(feature = "write")]
mod remap;
pub mod sections;
pub mod style;
mod validate;
#[cfg(feature = "write")]
mod write;

pub use container::{FORMAT_MAJOR, FORMAT_MINOR, PackageHash, SectionTag};
pub use error::PackageError;
pub use ids::{
    ARefId, ExprId, ImgId, NodeRef, StrId, StyleId, StyleSetId, TemplateId, TokenId, TplId,
};
pub use package::Package;
pub use profile::{PropId, Tag};
pub use sections::blobs::{
    ARefs, BlobRef, BlobSection, Face, FontRole, Fonts, Images, Img, Page, PageFormat, Scripts,
    Sprite,
};
pub use sections::manifest::{FaceFamily, FontRoles, Kind, Manifest, Tool};
pub use sections::styles::{Bound, Cond, CondKind, MAX_CELLS, MAX_CONDS, StyleSet, Styles};
pub use sections::templates::{
    ImageSrc, Layer, Node, NodeKind, Template, Templates, TextSrc, node_flags,
};
pub use sections::tokens::{Theme, Tokens};
pub use style::StyleRecord;
#[cfg(feature = "write")]
pub use write::PackageBuilder;
