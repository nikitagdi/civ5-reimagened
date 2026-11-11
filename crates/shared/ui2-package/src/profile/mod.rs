//! The UI profile: the subset of HTML and CSS the runtime understands.
//!
//! The schema types (`SchemaView`, `TypeSig`) are always available; the profile tables and parsers
//! need feature `profile`.

mod schema_view;

pub use schema_view::{SchemaView, TypeSig};
