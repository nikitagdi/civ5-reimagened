//! The model schema as seen by the package and the expression checker.
//!
//! `ui2-model` implements `SchemaView` for its `Schema`; this crate only consumes it, so the
//! dependency runs model to package and never back.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The type of a model path or an expression.
///
/// Postcard writes the variant index: new variants go at the end.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeSig {
    /// `true` or `false`.
    Bool,
    /// A 64-bit integer.
    Int,
    /// A float.
    Float,
    /// A plain string.
    Str,
    /// A game text (a `loc` key with arguments, or a ready string).
    Text,
    /// A stable list key of the named kind (a unit key, a city key).
    Key(String),
    /// A color.
    Color,
    /// One of the named values.
    Enum(Vec<String>),
    /// A list of the element type.
    List(Box<TypeSig>),
    /// A record of the named type.
    Struct(String),
    /// A value that may be `null`.
    Opt(Box<TypeSig>),
    /// A game asset reference.
    Asset,
}

impl fmt::Display for TypeSig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeSig::Bool => f.write_str("bool"),
            TypeSig::Int => f.write_str("int"),
            TypeSig::Float => f.write_str("float"),
            TypeSig::Str => f.write_str("str"),
            TypeSig::Text => f.write_str("text"),
            TypeSig::Key(kind) => write!(f, "key<{kind}>"),
            TypeSig::Color => f.write_str("color"),
            TypeSig::Enum(values) => write!(f, "enum({})", values.join("|")),
            TypeSig::List(item) => write!(f, "list<{item}>"),
            TypeSig::Struct(name) => write!(f, "struct {name}"),
            TypeSig::Opt(inner) => write!(f, "{inner}?"),
            TypeSig::Asset => f.write_str("asset"),
        }
    }
}

/// What the converter, the expression checker and the package loader ask of the model schema.
pub trait SchemaView {
    /// The type of a root of the model (`game`, `top`, `ui`, ...).
    fn root(&self, name: &str) -> Option<TypeSig>;
    /// The type of a field of a record type.
    fn field(&self, of: &TypeSig, name: &str) -> Option<TypeSig>;
    /// The name of the key field of a list's element type, when the schema declares one.
    fn list_key(&self, list: &TypeSig) -> Option<&str>;
    /// The named arguments of an intent, in declaration order.
    fn intent(&self, name: &str) -> Option<Vec<(String, TypeSig)>>;
    /// Whether script and markup may write the path (`js = true` fields).
    fn js_writable(&self, path: &str) -> bool;
    /// The model version `(major, minor)`.
    fn version(&self) -> (u16, u16);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let t = TypeSig::List(Box::new(TypeSig::Opt(Box::new(TypeSig::Key(
            "city".into(),
        )))));
        assert_eq!(t.to_string(), "list<key<city>?>");
        assert_eq!(
            TypeSig::Enum(vec!["a".into(), "b".into()]).to_string(),
            "enum(a|b)"
        );
    }
}
