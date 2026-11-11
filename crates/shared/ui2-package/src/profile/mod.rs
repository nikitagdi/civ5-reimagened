//! The UI profile: the subset of HTML and CSS the runtime understands.
//!
//! The schema types (`SchemaView`, `TypeSig`) and the id enums (`PropId`, `Tag`) are always
//! available; the profile tables and parsers need feature `profile`.

mod ids;
#[cfg(feature = "profile")]
mod props;
mod schema_view;

pub use ids::{PropId, Tag};
#[cfg(feature = "profile")]
pub use props::{
    ElementDef, ErrorDef, IgnoredDef, Profile, PropDef, SelectorDef, SlotDef, TableError, TokenDef,
    ValueKind,
};
pub use schema_view::{SchemaView, TypeSig};
