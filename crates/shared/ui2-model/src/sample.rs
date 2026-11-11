//! Samples: a model state written as JSON, checked against the schema. A skin ships `sample/<state>.json`; the
//! converter, the `?ui2sample=` mode of the client and the tests read it.
//!
//! ```json
//! { "model": { "top": { "gold": 312, "resources": [ { "id": "0:3", "name": "Iron" } ] } },
//!   "state": {}, "ui": {}, "anchors": { "city:3": [1320, 640] } }
//! ```
//!
//! JSON forms by schema type: numbers for integers, floats and colors, `true`/`false`, strings for `str` and
//! `asset`; `text` is a string (ready text) or `{"k": "key", "a": ["arg"]}`; `key<T>` is `"owner:id"` (or a plain
//! number); an enum is the variant name; `T?` is the value or `null`; a struct is an object; a list is an array
//! of element objects (the list key is read from the key field). A missing struct field takes its default, a
//! field the schema does not have is an error.
//!
//! Limits of this first version: only roots that exist in the schema are accepted. `ui` has no root in the
//! schema yet, so it must be empty; `state` is skin-defined and kept as it is, without a check.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value as Json;

use crate::model::Root;
use crate::schema::{FieldDef, RootId, Scalar, Schema, Type};
use crate::store::{ModelStore, Owner};
use crate::value::{AssetRef, Key, Text, Value, default_value, validate};

/// A parsed, checked sample.
#[derive(Clone, Debug)]
pub struct Sample {
    schema_hash: u64,
    roots: Vec<(RootId, Value)>,
    state: BTreeMap<String, Json>,
    anchors: BTreeMap<String, (f32, f32)>,
}

/// A sample that cannot be applied to a store.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SampleError {
    #[error("the sample was checked against another schema than the store's")]
    SchemaMismatch,
    #[error("root `{0}` has no typed model in this build")]
    UnknownRoot(String),
    #[error("root `{root}`: {msg}")]
    Root { root: String, msg: String },
    #[error("the store refused root `{root}`: {msg}")]
    Store { root: String, msg: String },
}

impl Sample {
    /// Read and check a sample.
    ///
    /// # Errors
    /// Every problem found, each with the path of the offending value (`model.top.gold: ...`).
    pub fn parse(schema: &Schema, text: &str) -> Result<Sample, Vec<String>> {
        let json: Json = serde_json::from_str(text).map_err(|e| vec![format!("sample: {e}")])?;
        let Json::Object(top) = json else {
            return Err(vec!["sample: expected an object".to_string()]);
        };
        let mut errs = Vec::new();
        let mut roots = Vec::new();
        let mut state = BTreeMap::new();
        let mut anchors = BTreeMap::new();
        for (k, v) in &top {
            match k.as_str() {
                "model" => model(schema, v, &mut roots, &mut errs),
                "state" => match v {
                    Json::Object(m) => state.extend(m.iter().map(|(k, v)| (k.clone(), v.clone()))),
                    _ => errs.push("state: expected an object".to_string()),
                },
                "ui" => match v {
                    Json::Object(m) if m.is_empty() => {}
                    Json::Object(m) => {
                        if schema.root_by_name("ui").is_some() {
                            errs.push("ui: the `ui` root is not supported in samples yet".into());
                        } else {
                            let first = m.keys().next().map_or("", String::as_str);
                            errs.push(format!("ui.{first}: the schema has no `ui` root"));
                        }
                    }
                    _ => errs.push("ui: expected an object".to_string()),
                },
                "anchors" => self::anchors(v, &mut anchors, &mut errs),
                other => errs.push(format!(
                    "{other}: unknown key (expected model, state, ui, anchors)"
                )),
            }
        }
        if errs.is_empty() {
            Ok(Sample {
                schema_hash: schema.hash(),
                roots,
                state,
                anchors,
            })
        } else {
            Err(errs)
        }
    }

    /// Write the roots of the sample into the store as the source would (the store diffs them when the host
    /// takes the frame).
    ///
    /// # Errors
    /// The store has another schema, or a root has no typed model or is refused.
    pub fn apply(&self, store: &mut ModelStore) -> Result<(), SampleError> {
        let schema = store.schema();
        if schema.hash() != self.schema_hash {
            return Err(SampleError::SchemaMismatch);
        }
        let mut w = store.writer(Owner::Source);
        for (rid, v) in &self.roots {
            let name = schema.root(*rid).name.as_str();
            match name {
                "meta" => put::<crate::generated::Meta>(&mut w, name, v)?,
                "game" => put::<crate::generated::Game>(&mut w, name, v)?,
                "top" => put::<crate::generated::Top>(&mut w, name, v)?,
                "research" => put::<crate::generated::Research>(&mut w, name, v)?,
                other => return Err(SampleError::UnknownRoot(other.to_string())),
            }
        }
        Ok(())
    }

    /// The screen points of the anchors, in CSS pixels.
    #[must_use]
    pub fn anchors(&self) -> &BTreeMap<String, (f32, f32)> {
        &self.anchors
    }

    /// The `state` object as written (skin-defined, not checked here).
    #[must_use]
    pub fn state(&self) -> &BTreeMap<String, Json> {
        &self.state
    }

    /// The checked root values, in the order of the file.
    #[must_use]
    pub fn roots(&self) -> &[(RootId, Value)] {
        &self.roots
    }
}

fn put<R: Root>(
    w: &mut crate::store::ModelWriter<'_>,
    name: &str,
    v: &Value,
) -> Result<(), SampleError> {
    let typed = R::from_value(v).map_err(|e| SampleError::Root {
        root: name.to_string(),
        msg: e.to_string(),
    })?;
    w.try_put(typed).map_err(|e| SampleError::Store {
        root: name.to_string(),
        msg: e.to_string(),
    })
}

fn model(schema: &Schema, v: &Json, out: &mut Vec<(RootId, Value)>, errs: &mut Vec<String>) {
    let Json::Object(m) = v else {
        errs.push("model: expected an object".to_string());
        return;
    };
    for (name, rv) in m {
        let path = format!("model.{name}");
        let Some(rid) = schema.root_by_name(name) else {
            errs.push(format!("{path}: unknown root"));
            continue;
        };
        let ty = Type::Struct(schema.root(rid).ty);
        let before = errs.len();
        let value = convert(schema, &ty, rv, &path, errs);
        if errs.len() == before {
            if let Err(e) = validate(schema, &ty, &value) {
                errs.push(format!("{path}: {e}"));
            } else {
                out.push((rid, value));
            }
        }
    }
}

fn anchors(v: &Json, out: &mut BTreeMap<String, (f32, f32)>, errs: &mut Vec<String>) {
    let Json::Object(m) = v else {
        errs.push("anchors: expected an object".to_string());
        return;
    };
    for (k, p) in m {
        let pair = p
            .as_array()
            .filter(|a| a.len() == 2)
            .and_then(|a| Some((a[0].as_f64()? as f32, a[1].as_f64()? as f32)));
        match pair {
            Some(xy) => {
                out.insert(k.clone(), xy);
            }
            None => errs.push(format!("anchors.{k}: expected [x, y]")),
        }
    }
}

fn mismatch(path: &str, expected: &str, got: &Json, errs: &mut Vec<String>) -> Value {
    let found = match got {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(_) => "a number",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    };
    errs.push(format!("{path}: expected {expected}, found {found}"));
    Value::None
}

fn convert(schema: &Schema, ty: &Type, j: &Json, path: &str, errs: &mut Vec<String>) -> Value {
    match ty {
        Type::Scalar(s) => scalar(*s, j, path, errs),
        Type::Key(_) => match j {
            Json::String(s) => match Key::parse_js(s) {
                Some(k) => Value::Key(k),
                None => {
                    errs.push(format!("{path}: key `{s}` is not of the form `owner:id`"));
                    Value::None
                }
            },
            Json::Number(n) => match n.as_u64() {
                Some(n) => Value::Key(Key(n)),
                None => mismatch(path, "a non-negative integer key", j, errs),
            },
            _ => mismatch(path, "a key (`\"owner:id\"`)", j, errs),
        },
        Type::Enum(e) => {
            let def = schema.enum_def(*e);
            match j {
                Json::String(s) => match def.variants.iter().position(|v| v == s) {
                    Some(i) => Value::Enum(i as u16),
                    None => {
                        errs.push(format!(
                            "{path}: `{s}` is not a variant of {} ({})",
                            def.name,
                            def.variants.join(", ")
                        ));
                        Value::None
                    }
                },
                _ => mismatch(path, "an enum variant name", j, errs),
            }
        }
        Type::Struct(t) => {
            let Json::Object(m) = j else {
                return mismatch(path, "an object", j, errs);
            };
            let td = schema.type_def(*t);
            for k in m.keys() {
                if !td.fields.iter().any(|f| &f.name == k) {
                    errs.push(format!(
                        "{path}.{k}: no such field in {} (fields: {})",
                        td.name,
                        td.fields
                            .iter()
                            .map(|f| f.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            let fields = td
                .fields
                .iter()
                .map(|f| field(schema, f, m.get(&f.name), path, errs))
                .collect();
            Value::Struct(fields)
        }
        Type::Option(inner) => match j {
            Json::Null => Value::None,
            other => convert(schema, inner, other, path, errs),
        },
        Type::List(l) => {
            let Json::Array(items) = j else {
                return mismatch(path, "an array", j, errs);
            };
            let elem = Type::Struct(l.elem);
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let p = format!("{path}[{i}]");
                if let Json::Object(m) = item
                    && !m.contains_key(&l.key)
                {
                    errs.push(format!("{p}.{}: list key is missing", l.key));
                    continue;
                }
                let v = convert(schema, &elem, item, &p, errs);
                let key = match &v {
                    Value::Struct(fs) => fs[l.key_index].as_key(),
                    _ => None,
                };
                if let Some(k) = key {
                    if out.iter().any(|(seen, _)| *seen == k) {
                        errs.push(format!("{p}.{}: duplicate list key {k}", l.key));
                    } else {
                        out.push((k, v));
                    }
                }
            }
            Value::List(out)
        }
    }
}

fn field(
    schema: &Schema,
    f: &FieldDef,
    j: Option<&Json>,
    path: &str,
    errs: &mut Vec<String>,
) -> Value {
    match j {
        Some(j) => convert(schema, &f.ty, j, &format!("{path}.{}", f.name), errs),
        None => default_value(schema, &f.ty),
    }
}

fn scalar(s: Scalar, j: &Json, path: &str, errs: &mut Vec<String>) -> Value {
    match (s, j) {
        (Scalar::Bool, Json::Bool(b)) => Value::Bool(*b),
        (Scalar::I8 | Scalar::I16 | Scalar::I32, Json::Number(n)) => match n.as_i64() {
            Some(n) => match i32::try_from(n).ok().filter(|v| match s {
                Scalar::I8 => i8::try_from(*v).is_ok(),
                Scalar::I16 => i16::try_from(*v).is_ok(),
                _ => true,
            }) {
                Some(n) => Value::I32(n),
                None => {
                    errs.push(format!("{path}: {n} is out of range for {}", s.name()));
                    Value::None
                }
            },
            None => mismatch(path, "an integer", j, errs),
        },
        (Scalar::U32 | Scalar::Color, Json::Number(n)) => match n.as_u64() {
            Some(n) => match u32::try_from(n) {
                Ok(n) => Value::U32(n),
                Err(_) => {
                    errs.push(format!("{path}: {n} is out of range for {}", s.name()));
                    Value::None
                }
            },
            None => mismatch(path, "an unsigned integer", j, errs),
        },
        (Scalar::F32, Json::Number(n)) => Value::F32(n.as_f64().unwrap_or(0.0) as f32),
        (Scalar::Str, Json::String(t)) => Value::Str(Arc::from(t.as_str())),
        (Scalar::Asset, Json::String(t)) => Value::Asset(AssetRef::new(t)),
        (Scalar::Text, Json::String(t)) => Value::Text(Text::raw(t)),
        (Scalar::Text, Json::Object(m)) => {
            let key = m.get("k").and_then(Json::as_str);
            let args: Option<Vec<Arc<str>>> = match m.get("a") {
                None => Some(Vec::new()),
                Some(Json::Array(a)) => a.iter().map(|x| x.as_str().map(Arc::from)).collect(),
                Some(_) => None,
            };
            if let Some(extra) = m.keys().find(|k| *k != "k" && *k != "a") {
                errs.push(format!("{path}.{extra}: text has only `k` and `a`"));
            }
            match (key, args) {
                (Some(k), Some(args)) => Value::Text(Text::Key {
                    key: Arc::from(k),
                    args,
                }),
                _ => {
                    errs.push(format!(
                        "{path}: expected {{\"k\": string, \"a\": [strings]}} for a localised text"
                    ));
                    Value::None
                }
            }
        }
        (Scalar::Bool, _) => mismatch(path, "a boolean", j, errs),
        (Scalar::Str | Scalar::Asset, _) => mismatch(path, "a string", j, errs),
        (Scalar::Text, _) => mismatch(path, "a string or {k, a}", j, errs),
        (Scalar::F32, _) => mismatch(path, "a number", j, errs),
        (Scalar::U32 | Scalar::Color, _) => mismatch(path, "an unsigned integer", j, errs),
        (Scalar::I8 | Scalar::I16 | Scalar::I32, _) => mismatch(path, "an integer", j, errs),
    }
}
