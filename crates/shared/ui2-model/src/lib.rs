//! UI v2 view model.
//!
//! One schema (`schema/ui-model.toml`) describes the data the UI shows: roots, structs, enums, keyed lists and
//! the intents the UI can send. The Rust types, the path table and the JS typings are generated from it and
//! committed. The model is read-only for skins and changes by deltas once per frame.

#![forbid(unsafe_code)]

pub mod schema;

pub use schema::{
    EnumDef, EnumId, FieldDef, FieldId, FieldRef, IntentDef, ListType, PathEntry, Root, RootId,
    RootKind, Scalar, Schema, SchemaError, StructDef, Type, TypeId, Version,
};
