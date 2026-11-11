//! Intents: the catalogue of the schema (`[intent.*]`) applied to what a skin, a script, a hotkey or a map click
//! sent as a [`WireIntent`]. Parsing checks the form only: the name is in the catalogue, every argument is
//! given once, with the type the catalogue says. Whether the intent makes sense in the current game is checked
//! by the routes.

use std::sync::Arc;

use ui2_model::schema::{IntentDef, Schema, Type};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key, Value, validate};

/// An intent that does not have the form its catalogue entry asks for. The text names the intent and the
/// argument and is meant for the skin author.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IntentError {
    #[error("unknown intent `{0}`")]
    Unknown(Arc<str>),
    #[error("intent `{intent}`: missing argument `{arg}`")]
    Missing { intent: Arc<str>, arg: String },
    #[error("intent `{intent}`: unknown argument `{arg}`")]
    Unexpected { intent: Arc<str>, arg: Arc<str> },
    #[error("intent `{intent}`: argument `{arg}` is given twice")]
    Duplicate { intent: Arc<str>, arg: Arc<str> },
    #[error("intent `{intent}`: {why}")]
    ModIntent { intent: Arc<str>, why: String },
    #[error("intent `{intent}`: a mod may send only its own `mods.<id>.<name>` intents")]
    ModCatalog { intent: Arc<str> },
    #[error("intent `{intent}`: argument `{arg}`: {msg}")]
    BadArg {
        intent: Arc<str>,
        arg: String,
        msg: String,
    },
}

/// A checked intent: the catalogue entry and the argument values in the order of the catalogue, in the
/// canonical forms (an enum is its index, a key is a [`Key`], an absent optional is [`Value::None`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    def: &'static IntentDef,
    args: Vec<Value>,
    /// Who raised it.
    pub origin: Origin,
}

impl Intent {
    /// Check a wire intent against the catalogue of `schema`. Text is accepted where the type has a text form:
    /// an enum by variant name, a key as `"owner:id"` (the forms JS uses).
    ///
    /// # Errors
    /// The name is not in the catalogue, an argument is missing, unknown or given twice, or a value does not
    /// have the type of its argument. An optional argument may be left out.
    pub fn from_wire(schema: &'static Schema, wire: &WireIntent) -> Result<Intent, IntentError> {
        let def = schema
            .intents()
            .iter()
            .find(|i| *i.name == *wire.name)
            .ok_or_else(|| IntentError::Unknown(wire.name.clone()))?;
        for (i, (name, _)) in wire.args.iter().enumerate() {
            if !def.args.iter().any(|a| *a.name == **name) {
                return Err(IntentError::Unexpected {
                    intent: wire.name.clone(),
                    arg: name.clone(),
                });
            }
            if wire.args[..i].iter().any(|(n, _)| n == name) {
                return Err(IntentError::Duplicate {
                    intent: wire.name.clone(),
                    arg: name.clone(),
                });
            }
        }
        let mut args = Vec::with_capacity(def.args.len());
        for a in &def.args {
            let given = wire
                .args
                .iter()
                .find(|(n, _)| **n == *a.name)
                .map(|(_, v)| v);
            let value = match given {
                Some(v) => coerce(schema, &a.ty, v).map_err(|msg| IntentError::BadArg {
                    intent: wire.name.clone(),
                    arg: a.name.clone(),
                    msg,
                })?,
                None if matches!(a.ty, Type::Option(_)) => Value::None,
                None => {
                    return Err(IntentError::Missing {
                        intent: wire.name.clone(),
                        arg: a.name.clone(),
                    });
                }
            };
            args.push(value);
        }
        Ok(Intent {
            def,
            args,
            origin: wire.origin,
        })
    }

    /// The name, `unit.action`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        &self.def.name
    }

    /// The value of an argument.
    #[must_use]
    pub fn get(&self, arg: &str) -> Option<&Value> {
        let i = self.def.args.iter().position(|a| a.name == arg)?;
        self.args.get(i)
    }

    /// A key argument; `None` for an absent optional or an argument that is not a key.
    #[must_use]
    pub fn key(&self, arg: &str) -> Option<Key> {
        match self.get(arg)? {
            Value::Key(k) => Some(*k),
            _ => None,
        }
    }

    /// An integer argument.
    #[must_use]
    pub fn int(&self, arg: &str) -> Option<i32> {
        match self.get(arg)? {
            Value::I32(n) => Some(*n),
            _ => None,
        }
    }

    /// A boolean argument.
    #[must_use]
    pub fn flag(&self, arg: &str) -> Option<bool> {
        match self.get(arg)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// A text (`str`) argument.
    #[must_use]
    pub fn text(&self, arg: &str) -> Option<&str> {
        match self.get(arg)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// An enum argument: the index of the variant.
    #[must_use]
    pub fn variant(&self, arg: &str) -> Option<u16> {
        match self.get(arg)? {
            Value::Enum(n) => Some(*n),
            _ => None,
        }
    }

    /// A `Plot` argument: `(x, y)`.
    #[must_use]
    pub fn plot(&self, arg: &str) -> Option<(i16, i16)> {
        match self.get(arg)? {
            Value::Struct(f) => match f.as_slice() {
                [Value::I32(x), Value::I32(y)] => {
                    Some((i16::try_from(*x).ok()?, i16::try_from(*y).ok()?))
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Bring a value to the canonical form of `ty`, or say why it does not fit.
pub(crate) fn coerce(schema: &Schema, ty: &Type, v: &Value) -> Result<Value, String> {
    let out = match (ty, v) {
        (Type::Option(_), Value::None) => Value::None,
        (Type::Option(inner), other) => coerce(schema, inner, other)?,
        (Type::Enum(e), Value::Str(name)) => {
            let def = schema.enum_def(*e);
            let i = def
                .variants
                .iter()
                .position(|x| **x == **name)
                .ok_or_else(|| format!("`{name}` is not a variant of `{}`", def.name))?;
            Value::Enum(i as u16)
        }
        (Type::Key(_), Value::Str(s)) => Value::Key(
            Key::parse_js(s).ok_or_else(|| format!("`{s}` is not a key (expected `owner:id`)"))?,
        ),
        (Type::Struct(t), Value::Struct(fields)) => {
            let td = schema.type_def(*t);
            if fields.len() != td.fields.len() {
                return Err(format!(
                    "`{}` has {} fields, {} given",
                    td.name,
                    td.fields.len(),
                    fields.len()
                ));
            }
            let mut out = Vec::with_capacity(fields.len());
            for (f, fv) in td.fields.iter().zip(fields) {
                out.push(coerce(schema, &f.ty, fv).map_err(|m| format!("{}: {m}", f.name))?);
            }
            Value::Struct(out)
        }
        (_, other) => other.clone(),
    };
    validate(schema, ty, &out).map_err(|e| e.to_string())?;
    Ok(out)
}
