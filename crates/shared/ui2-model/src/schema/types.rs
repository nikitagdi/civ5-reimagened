//! Resolved schema types: scalars, enums, structs, lists, the flat path table.

use std::fmt;

/// Maximum nesting of keyed lists (list inside list inside ...). Keeps `Keys` a fixed-size array.
pub const MAX_LIST_DEPTH: usize = 4;

/// Index into the flat path table of one schema build. Not stable between schema versions: packages store
/// path strings and resolve them when loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldId(pub u32);

impl FieldId {
    /// The id of a child that sits `offset` slots after the first slot of the body of this node (see
    /// [`StructDef::offsets`]). Generated code calls it with literal offsets.
    #[inline]
    #[must_use]
    pub const fn child(self, offset: u32) -> FieldId {
        FieldId(self.0 + 1 + offset)
    }
}

/// Index of a model root (`[model.<name>]`) in schema order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RootId(pub u16);

/// Index into [`Schema::types`](super::Schema::types).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// Index into [`Schema::enums`](super::Schema::enums).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumId(pub u32);

/// Built-in scalar types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scalar {
    Bool,
    I8,
    I16,
    I32,
    U32,
    F32,
    /// Raw text (`Arc<str>`).
    Str,
    /// Localisable text (`Text`).
    Text,
    /// Game asset id (`AssetRef`).
    Asset,
    /// RGBA, stored as `u32`.
    Color,
}

impl Scalar {
    /// The name used in schema sources.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Scalar::Bool => "bool",
            Scalar::I8 => "i8",
            Scalar::I16 => "i16",
            Scalar::I32 => "i32",
            Scalar::U32 => "u32",
            Scalar::F32 => "f32",
            Scalar::Str => "str",
            Scalar::Text => "text",
            Scalar::Asset => "asset",
            Scalar::Color => "color",
        }
    }

    /// Parse a schema-source name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Scalar> {
        Some(match s {
            "bool" => Scalar::Bool,
            "i8" => Scalar::I8,
            "i16" => Scalar::I16,
            "i32" => Scalar::I32,
            "u32" => Scalar::U32,
            "f32" => Scalar::F32,
            "str" => Scalar::Str,
            "text" => Scalar::Text,
            "asset" => Scalar::Asset,
            "color" => Scalar::Color,
            _ => return None,
        })
    }
}

/// A keyed list type: `{ list = "Elem", key = "field" }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListType {
    /// The element struct.
    pub elem: TypeId,
    /// Name of the key field of the element.
    pub key: String,
    /// Index of the key field in the element struct.
    pub key_index: usize,
    /// Type of the key field: `key<_>`, an integer, an enum or `str`.
    pub key_ty: Box<Type>,
}

/// A resolved field type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Scalar(Scalar),
    /// `key<entity>`: a stable entity key (`Key(u64)`).
    Key(String),
    Enum(EnumId),
    Struct(TypeId),
    /// `T?`; `T` is never an option or a list.
    Option(Box<Type>),
    List(ListType),
}

impl Type {
    /// The struct behind `Struct(t)` and `Option(Struct(t))`.
    #[must_use]
    pub fn struct_id(&self) -> Option<TypeId> {
        match self {
            Type::Struct(t) => Some(*t),
            Type::Option(inner) => inner.struct_id(),
            _ => None,
        }
    }
}

/// One field of a struct, root or intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Type,
    /// `js = true`: writable from JS and markup (only in the `ui` root).
    pub js: bool,
}

/// A struct type (named `[type.*]` or the struct of a root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// True for the struct of `[model.<root>]`.
    pub is_root: bool,
    /// For a struct used as a list element: the index of its key field.
    pub key_index: Option<usize>,
    /// Slot offset of every field relative to the node holding the struct: the field's [`FieldId`] is
    /// `node + 1 + offsets[i]`. Depends on the struct only, so generated code uses it as a constant.
    pub offsets: Vec<u32>,
    /// Number of path slots below the node holding the struct.
    pub body_len: u32,
}

impl Root {
    /// The type of the value of the whole root.
    #[must_use]
    pub fn value_type(&self) -> Type {
        match &self.shape {
            RootShape::Struct => Type::Struct(self.ty),
            RootShape::Optional => Type::Option(Box::new(Type::Struct(self.ty))),
            RootShape::List(l) => Type::List(l.clone()),
        }
    }
}

/// An enum: a closed set of names, stored as a `u16` index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

/// An intent: a named command with typed arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentDef {
    pub name: String,
    pub args: Vec<FieldDef>,
}

/// Who may write a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RootKind {
    /// Game data: written by the source (the replica adapter, a server on stage 3).
    Data,
    /// The `ui` root: written by the actions module.
    Ui,
}

/// The form of a root: a struct, an optional struct (absent while nothing applies, like the selected unit) or
/// a keyed list (the notes, the chat lines).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootShape {
    /// `[model.x]` with fields: always present.
    Struct,
    /// `[model.x]` with `_optional = true`: `None` until the source has something to show.
    Optional,
    /// `[model.x]` with `_list = "Elem"` and `_key = "field"`: a list at the top of the model.
    List(ListType),
}

/// A model root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    /// The root struct, or the element struct of a list root.
    pub ty: TypeId,
    pub shape: RootShape,
    /// The path slot of the root itself.
    pub field: FieldId,
    pub kind: RootKind,
}

/// One row of the flat path table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathEntry {
    /// `top.gold`, `top.resources[].name`.
    pub path: String,
    pub ty: Type,
    pub root: RootId,
    /// Field indices from the root struct down to this node (empty for a root).
    pub steps: Vec<u16>,
    /// The key field name when `ty` is a list.
    pub list_key: Option<String>,
    /// Writable from JS and markup.
    pub js: bool,
    /// One past the last slot of the subtree of this node (ids `id+1 .. end` are descendants).
    pub end: u32,
}

/// The result of resolving a path string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldRef {
    pub id: FieldId,
    pub ty: Type,
}

/// `MAJOR.MINOR` of a schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    /// Parse `"1.3"`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Version> {
        let (a, b) = s.split_once('.')?;
        let digits = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit());
        if !digits(a) || !digits(b) {
            return None;
        }
        Some(Version {
            major: a.parse().ok()?,
            minor: b.parse().ok()?,
        })
    }

    /// A client of this version runs a package built for `pkg`: same major, package minor not newer.
    #[must_use]
    pub fn accepts(self, pkg: Version) -> bool {
        self.major == pkg.major && pkg.minor <= self.minor
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}
