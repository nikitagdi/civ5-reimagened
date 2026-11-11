//! Dynamic values: the representation used by deltas, fragments, JS and the wire.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::schema::{Scalar, Schema, Type};

/// A stable entity key (`key<unit>`, list keys). For game entities it is `(owner << 32) | id`; for list keys
/// that are integers, enums or strings it is derived from the key field (see [`Key::from_i32`] and friends).
///
/// In JS a key is the string `"hi:lo"` (high and low 32 bits in decimal), because a `u64` does not fit a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub u64);

impl Key {
    /// `(owner << 32) | id`.
    #[must_use]
    pub const fn new(owner: u32, id: u32) -> Key {
        Key(((owner as u64) << 32) | id as u64)
    }

    /// Key of an integer list key field (`i8`, `i16`, `i32`).
    #[must_use]
    pub const fn from_i32(v: i32) -> Key {
        Key(v as u32 as u64)
    }

    /// Key of a `u32` list key field.
    #[must_use]
    pub const fn from_u32(v: u32) -> Key {
        Key(v as u64)
    }

    /// Key of an enum list key field.
    #[must_use]
    pub const fn from_enum(v: u16) -> Key {
        Key(v as u64)
    }

    /// Key of a `str` list key field: the 64-bit FNV-1a hash of the text. A collision inside one list is
    /// reported as a duplicate key by validation.
    #[must_use]
    pub fn from_text(s: &str) -> Key {
        Key(crate::schema::fnv1a(s.as_bytes()))
    }

    /// The JS form `"hi:lo"`.
    #[must_use]
    pub fn to_js(self) -> String {
        self.to_string()
    }

    /// Parse the JS form.
    #[must_use]
    pub fn parse_js(s: &str) -> Option<Key> {
        let (hi, lo) = s.split_once(':')?;
        Some(Key::new(hi.parse().ok()?, lo.parse().ok()?))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.0 >> 32, self.0 & 0xffff_ffff)
    }
}

/// An id of a game asset (`icon:UNIT_ATLAS_1:12`), resolved by the render layers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AssetRef(pub Arc<str>);

impl AssetRef {
    /// Build from text.
    #[must_use]
    pub fn new(s: &str) -> AssetRef {
        AssetRef(Arc::from(s))
    }
}

/// Text shown to the player: a localisation key with arguments (translated by the runtime, so a language switch
/// needs no new model) or ready text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Text {
    Key { key: Arc<str>, args: Vec<Arc<str>> },
    Raw(Arc<str>),
}

impl Default for Text {
    fn default() -> Text {
        Text::Raw(Arc::from(""))
    }
}

impl Text {
    /// Ready text.
    #[must_use]
    pub fn raw(s: &str) -> Text {
        Text::Raw(Arc::from(s))
    }

    /// A localisation key with arguments.
    #[must_use]
    pub fn key(key: &str, args: &[&str]) -> Text {
        Text::Key {
            key: Arc::from(key),
            args: args.iter().map(|a| Arc::from(*a)).collect(),
        }
    }
}

/// The dynamic representation of any model value. Small integers are `I32`, colors are `U32`; the enum is the
/// index of the variant. `None` is an absent optional (an optional struct is the bare `Struct`).
#[derive(Clone, Debug)]
pub enum Value {
    None,
    Bool(bool),
    I32(i32),
    U32(u32),
    F32(f32),
    Str(Arc<str>),
    Text(Text),
    Asset(AssetRef),
    Enum(u16),
    Key(Key),
    Struct(Vec<Value>),
    List(Vec<(Key, Value)>),
}

impl PartialEq for Value {
    /// `f32` compares bitwise (a share that shivers by one ulp is a change; `NaN` equals itself).
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::None, Value::None) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::I32(a), Value::I32(b)) => a == b,
            (Value::U32(a), Value::U32(b)) => a == b,
            (Value::F32(a), Value::F32(b)) => a.to_bits() == b.to_bits(),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Asset(a), Value::Asset(b)) => a == b,
            (Value::Enum(a), Value::Enum(b)) => a == b,
            (Value::Key(a), Value::Key(b)) => a == b,
            (Value::Struct(a), Value::Struct(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            _ => false,
        }
    }
}

/// A borrowed value, as handed out by the store.
pub type ValueRef<'a> = &'a Value;

impl Value {
    /// The variant name, for error texts.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Value::None => "none",
            Value::Bool(_) => "bool",
            Value::I32(_) => "i32",
            Value::U32(_) => "u32",
            Value::F32(_) => "f32",
            Value::Str(_) => "str",
            Value::Text(_) => "text",
            Value::Asset(_) => "asset",
            Value::Enum(_) => "enum",
            Value::Key(_) => "key",
            Value::Struct(_) => "struct",
            Value::List(_) => "list",
        }
    }

    /// The fields of a struct value, checking the count.
    ///
    /// # Errors
    /// Not a struct, or a different field count.
    pub fn as_struct(&self, fields: usize) -> Result<&[Value], ValueError> {
        match self {
            Value::Struct(v) if v.len() == fields => Ok(v),
            Value::Struct(v) => Err(ValueError::Arity {
                expected: fields,
                found: v.len(),
            }),
            other => Err(ValueError::mismatch("struct", other)),
        }
    }

    /// The key of a list element from its key field value.
    #[must_use]
    pub fn as_key(&self) -> Option<Key> {
        match self {
            Value::Key(k) => Some(*k),
            Value::I32(v) => Some(Key::from_i32(*v)),
            Value::U32(v) => Some(Key::from_u32(*v)),
            Value::Enum(v) => Some(Key::from_enum(*v)),
            Value::Str(s) => Some(Key::from_text(s)),
            _ => None,
        }
    }
}

/// A value that does not fit its type.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ValueError {
    #[error("expected {expected}, found {found}")]
    Mismatch {
        expected: &'static str,
        found: &'static str,
    },
    #[error("struct with {found} fields where {expected} are expected")]
    Arity { expected: usize, found: usize },
    #[error("number {value} is out of range for {ty}")]
    Range { value: i64, ty: &'static str },
    #[error("enum index {value} is out of range ({count} variants)")]
    EnumIndex { value: u16, count: usize },
    #[error("duplicate list key {0}")]
    DuplicateKey(Key),
    #[error("list key {listed} does not match the key field of its element ({actual})")]
    KeyMismatch { listed: Key, actual: Key },
}

impl ValueError {
    pub(crate) fn mismatch(expected: &'static str, found: &Value) -> ValueError {
        ValueError::Mismatch {
            expected,
            found: found.kind(),
        }
    }
}

/// The default value of a type: zero, empty, `None`, the first enum variant.
#[must_use]
pub fn default_value(schema: &Schema, ty: &Type) -> Value {
    match ty {
        Type::Scalar(s) => match s {
            Scalar::Bool => Value::Bool(false),
            Scalar::I8 | Scalar::I16 | Scalar::I32 => Value::I32(0),
            Scalar::U32 | Scalar::Color => Value::U32(0),
            Scalar::F32 => Value::F32(0.0),
            Scalar::Str => Value::Str(Arc::from("")),
            Scalar::Text => Value::Text(Text::default()),
            Scalar::Asset => Value::Asset(AssetRef::default()),
        },
        Type::Key(_) => Value::Key(Key(0)),
        Type::Enum(_) => Value::Enum(0),
        Type::Struct(t) => Value::Struct(
            schema
                .type_def(*t)
                .fields
                .iter()
                .map(|f| default_value(schema, &f.ty))
                .collect(),
        ),
        Type::Option(_) => Value::None,
        Type::List(_) => Value::List(Vec::new()),
    }
}

/// Check that a value has the shape of a type: scalars in range, enum indices known, struct arity, list keys
/// unique and equal to the key field of their element.
///
/// # Errors
/// The first mismatch.
pub fn validate(schema: &Schema, ty: &Type, v: &Value) -> Result<(), ValueError> {
    match (ty, v) {
        (Type::Scalar(Scalar::Bool), Value::Bool(_))
        | (Type::Scalar(Scalar::F32), Value::F32(_))
        | (Type::Scalar(Scalar::Str), Value::Str(_))
        | (Type::Scalar(Scalar::Text), Value::Text(_))
        | (Type::Scalar(Scalar::Asset), Value::Asset(_))
        | (Type::Scalar(Scalar::U32 | Scalar::Color), Value::U32(_))
        | (Type::Scalar(Scalar::I32), Value::I32(_))
        | (Type::Key(_), Value::Key(_)) => Ok(()),
        (Type::Scalar(Scalar::I8), Value::I32(n)) => {
            range(*n, i8::MIN.into(), i8::MAX.into(), "i8")
        }
        (Type::Scalar(Scalar::I16), Value::I32(n)) => {
            range(*n, i16::MIN.into(), i16::MAX.into(), "i16")
        }
        (Type::Enum(e), Value::Enum(n)) => {
            let count = schema.enum_def(*e).variants.len();
            if usize::from(*n) < count {
                Ok(())
            } else {
                Err(ValueError::EnumIndex { value: *n, count })
            }
        }
        (Type::Struct(t), Value::Struct(fs)) => {
            let td = schema.type_def(*t);
            if fs.len() != td.fields.len() {
                return Err(ValueError::Arity {
                    expected: td.fields.len(),
                    found: fs.len(),
                });
            }
            for (f, fv) in td.fields.iter().zip(fs) {
                validate(schema, &f.ty, fv)?;
            }
            Ok(())
        }
        (Type::Option(_), Value::None) => Ok(()),
        (Type::Option(inner), other) => validate(schema, inner, other),
        (Type::List(l), Value::List(items)) => {
            let elem = Type::Struct(l.elem);
            let mut seen = HashSet::with_capacity(items.len());
            for (k, ev) in items {
                validate(schema, &elem, ev)?;
                let Value::Struct(fs) = ev else {
                    return Err(ValueError::mismatch("struct", ev));
                };
                let actual = fs[l.key_index].as_key().unwrap_or(Key(0));
                if actual != *k {
                    return Err(ValueError::KeyMismatch { listed: *k, actual });
                }
                if !seen.insert(*k) {
                    return Err(ValueError::DuplicateKey(*k));
                }
            }
            Ok(())
        }
        (ty, v) => Err(ValueError::Mismatch {
            expected: type_kind(ty),
            found: v.kind(),
        }),
    }
}

fn range(n: i32, lo: i32, hi: i32, ty: &'static str) -> Result<(), ValueError> {
    if (lo..=hi).contains(&n) {
        Ok(())
    } else {
        Err(ValueError::Range {
            value: i64::from(n),
            ty,
        })
    }
}

fn type_kind(ty: &Type) -> &'static str {
    match ty {
        Type::Scalar(s) => match s {
            Scalar::Bool => "bool",
            Scalar::I8 | Scalar::I16 | Scalar::I32 => "i32",
            Scalar::U32 | Scalar::Color => "u32",
            Scalar::F32 => "f32",
            Scalar::Str => "str",
            Scalar::Text => "text",
            Scalar::Asset => "asset",
        },
        Type::Key(_) => "key",
        Type::Enum(_) => "enum",
        Type::Struct(_) => "struct",
        Type::Option(_) => "optional",
        Type::List(_) => "list",
    }
}
