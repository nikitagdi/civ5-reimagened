//! A schema made of plain maps, for tests of the expression checker, the attribute checker and the
//! package's path checks.
#![allow(dead_code)]

pub mod mini;
pub mod raw;

use std::collections::{BTreeMap, BTreeSet};

use ui2_package::profile::{SchemaView, TypeSig};

/// `int`, `list<struct a.b[]>`, `enum(a|b)`, `key<city>`, `struct p?` as `TypeSig`.
pub fn parse_type(s: &str) -> TypeSig {
    if let Some(inner) = s.strip_suffix('?') {
        return TypeSig::Opt(Box::new(parse_type(inner)));
    }
    match s {
        "bool" => TypeSig::Bool,
        "int" => TypeSig::Int,
        "float" => TypeSig::Float,
        "str" => TypeSig::Str,
        "text" => TypeSig::Text,
        "color" => TypeSig::Color,
        "asset" => TypeSig::Asset,
        _ => {
            if let Some(name) = s.strip_prefix("struct ") {
                TypeSig::Struct(name.to_string())
            } else if let Some(kind) = s.strip_prefix("key<").and_then(|r| r.strip_suffix('>')) {
                TypeSig::Key(kind.to_string())
            } else if let Some(list) = s.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
                TypeSig::List(Box::new(parse_type(list)))
            } else if let Some(values) = s.strip_prefix("enum(").and_then(|r| r.strip_suffix(')')) {
                TypeSig::Enum(values.split('|').map(str::to_string).collect())
            } else {
                panic!("unknown type {s:?}")
            }
        }
    }
}

/// The schema as a flat map: a struct type `struct p` has its field `f` at the path `p.f`; a
/// root is the first segment of any path.
#[derive(Default)]
pub struct MapView {
    pub types: BTreeMap<String, TypeSig>,
    pub keys: BTreeMap<String, String>,
    pub intents: BTreeMap<String, Vec<(String, TypeSig)>>,
    pub writable: BTreeSet<String>,
}

impl MapView {
    pub fn from_types<'a>(types: impl IntoIterator<Item = (&'a String, &'a String)>) -> Self {
        Self {
            types: types
                .into_iter()
                .map(|(k, v)| (k.clone(), parse_type(v)))
                .collect(),
            ..Self::default()
        }
    }

    pub fn ty(mut self, path: &str, ty: &str) -> Self {
        self.types.insert(path.to_string(), parse_type(ty));
        self
    }

    pub fn key(mut self, elem: &str, field: &str) -> Self {
        self.keys.insert(elem.to_string(), field.to_string());
        self
    }

    pub fn intent(mut self, name: &str, args: &[(&str, &str)]) -> Self {
        self.intents.insert(
            name.to_string(),
            args.iter()
                .map(|(n, t)| ((*n).to_string(), parse_type(t)))
                .collect(),
        );
        self
    }

    pub fn writable(mut self, path: &str) -> Self {
        self.writable.insert(path.to_string());
        self
    }
}

impl SchemaView for MapView {
    fn root(&self, name: &str) -> Option<TypeSig> {
        let prefix = format!("{name}.");
        self.types
            .keys()
            .any(|k| k.starts_with(&prefix))
            .then(|| TypeSig::Struct(name.to_string()))
    }

    fn field(&self, of: &TypeSig, name: &str) -> Option<TypeSig> {
        let of = match of {
            TypeSig::Opt(inner) => inner.as_ref(),
            other => other,
        };
        let TypeSig::Struct(path) = of else {
            return None;
        };
        self.types.get(&format!("{path}.{name}")).cloned()
    }

    fn list_key(&self, list: &TypeSig) -> Option<&str> {
        match list {
            TypeSig::List(elem) => match elem.as_ref() {
                TypeSig::Struct(path) => self.keys.get(path).map(String::as_str),
                _ => None,
            },
            _ => None,
        }
    }

    fn intent(&self, name: &str) -> Option<Vec<(String, TypeSig)>> {
        self.intents.get(name).cloned()
    }

    fn js_writable(&self, path: &str) -> bool {
        self.writable.contains(path)
    }

    fn version(&self) -> (u16, u16) {
        (1, 0)
    }
}
