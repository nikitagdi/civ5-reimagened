//! UI v2 view model.
//!
//! One schema (`schema/ui-model.toml`) describes the data the UI shows: roots, structs, enums, keyed lists and
//! the intents the UI can send. The Rust types, the path table and the JS typings are generated from it and
//! committed. The model is read-only for skins and changes by deltas once per frame.

#![forbid(unsafe_code)]

pub mod codegen;
pub mod delta;
pub mod diff;
#[path = "gen/mod.rs"]
pub mod generated;
pub mod model;
pub mod sample;
pub mod schema;
mod schema_view;
pub mod state;
pub mod store;
mod umd1;
pub mod value;
pub mod watch;
pub mod wire;

pub use delta::{Cursor, FrameDelta, Keys, Op, Ops};
pub use diff::{ListStep, diff_root, list_steps};
/// The generated types, under the name the specs use (gen is a keyword in edition 2024).
pub use generated as r#gen;
pub use model::{Keyed, KeyedList, Model, Root};
pub use sample::{Sample, SampleError};
pub use schema::{
    EnumDef, EnumId, FieldDef, FieldId, FieldRef, IntentDef, ListType, PathEntry, Root as RootDef,
    RootId, RootKind, Scalar, Schema, SchemaError, StructDef, Type, TypeId, Version,
};
pub use state::{ApplyError, ModelState};
pub use store::{ModelStore, ModelWriter, Owner, StoreError};
pub use value::{AssetRef, Key, Text, Value, ValueError, ValueRef, default_value, validate};
pub use watch::WatchSet;
