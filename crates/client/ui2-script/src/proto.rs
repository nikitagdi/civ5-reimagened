//! The messages between the supervisor and a script worker: one JSON text per message, at most one per context
//! and frame in each direction.
//!
//! Main thread to worker (`t` is the kind):
//!
//! | `t` | fields |
//! |---|---|
//! | `init` | `api`, `ctx`, `files: [{path, code}]`, `model` (UMD1 snapshot), `schema` (decoding tables, below), `reads` (watched prefixes), `ns: {state, intents}`, `settings: [[key, json text]]`, `env`, `limits`, `dev` |
//! | `frame` | `seq`, `applied` (the last `out.n` applied before this frame's delta was taken), `model` (UMD1 delta or `null`), `events: [{name, arg, node}]`, `env` (only when it changed) |
//! | `stop` | — |
//!
//! Worker to main thread:
//!
//! | `t` | fields |
//! |---|---|
//! | `ready` | `ctx`, `handlers` (names given to `ui.on`), `ms` (load time) |
//! | `fail` | `ctx`, `error: {message, file, line}`: a file threw while loading |
//! | `out` | `n` (the worker's out counter), `ack` (the `seq` of the frame answered, `null` for one the worker started itself), `state: [{p, v}]`, `intents: [{name, args: {..}}]`, `cmds: [{island, key, op, ..}]`, `settings: [{key, value}]`, `log: [{level, text, file, line, key}]` (`key`: what an error is about, written once), `busy_ms` |
//!
//! `env` is `{lang, viewport: [w, h], scale, cell, skin}`. A state path is `state.<path>` for the context's own
//! state (the skin's `state.*`, a mod's `modstate.<id>.*`) or `ui.<path>` for a `js` field of the `ui` root.
//! An island command is `{island, key, op: "text", value}` (a string, or `{loc, args}`), `{.., op: "cls", name,
//! on}`, `{.., op: "style", prop, value}` or `{.., op: "show", on}`.
//!
//! The `schema` of `init` lets the worker decode UMD1 without the generated module: `{hash, parts: [{base,
//! roots: [[id, name, type]], enums: {name: [variant]}, types: {name: [[field, type]]}, paths: [[path, type,
//! list key or null, js]]}]}`. The first part is the model schema (base 0); each mounted fragment the context
//! watches is another part, its field ids shifted by `base` and its one root numbered `id`. Types are written as
//! in `model.gen.js`: a scalar name, `"key"`, `["enum", E]`, `["struct", T]`, `["opt", type]`, `["list", T]`.

use std::fmt::Write as _;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value as Json};
use ui2_model::schema::{Scalar, Type};
use ui2_model::value::{AssetRef, Key, Text};
use ui2_model::wire::{IslandCmd, IslandOp, ScriptEvent};
use ui2_model::{Schema, Value};

use crate::{
    LogLevel, LogLine, SchemaPart, ScriptBundle, ScriptEnv, ScriptInit, StateNs, UI_SCRIPT_API,
};

/// The slowest handler call that is not reported, the most log lines per second, the shortest timer.
pub const SLOW_HANDLER_MS: u32 = 50;
pub const LOG_LINES_PER_S: u32 = 20;
pub const TIMER_MIN_MS: u32 = 50;

fn json_str(s: &str) -> String {
    Json::String(s.to_string()).to_string()
}

fn env_json(env: &ScriptEnv) -> String {
    let mut o = Map::new();
    o.insert("lang".into(), Json::from(env.lang.as_str()));
    o.insert(
        "viewport".into(),
        Json::from(vec![env.viewport.0, env.viewport.1]),
    );
    o.insert("scale".into(), Json::from(f64::from(env.scale)));
    o.insert("cell".into(), Json::from(env.cell));
    o.insert("skin".into(), Json::from(env.skin.as_str()));
    Json::Object(o).to_string()
}

/// The `init` text.
pub(crate) fn encode_init(ctx: &str, bundle: &ScriptBundle, init: &ScriptInit) -> String {
    let mut out = String::with_capacity(init.model_umd1.len() + 32 * 1024);
    let _ = write!(
        out,
        "{{\"t\":\"init\",\"api\":{},\"ctx\":{},\"files\":[",
        json_str(UI_SCRIPT_API),
        json_str(ctx)
    );
    for (i, (path, code)) in bundle.files.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"path\":{},\"code\":{}}}",
            json_str(path),
            json_str(code)
        );
    }
    out.push_str("],\"model\":");
    out.push_str(&init.model_umd1);
    out.push_str(",\"schema\":");
    tables(&mut out, &init.parts);
    let reads: Vec<Json> = init
        .prefixes
        .iter()
        .map(|p| Json::from(p.as_str()))
        .collect();
    let _ = write!(out, ",\"reads\":{}", Json::Array(reads));
    let state = bundle.state_ns.namespace().root_name();
    let intents = bundle
        .intent_prefix
        .as_deref()
        .map_or(Json::Null, Json::from);
    let _ = write!(
        out,
        ",\"ns\":{{\"state\":{},\"intents\":{}}}",
        json_str(&state),
        intents
    );
    let settings: Vec<Json> = init
        .settings
        .iter()
        .map(|(k, v)| Json::from(vec![k.as_str(), v.as_str()]))
        .collect();
    let _ = write!(out, ",\"settings\":{}", Json::Array(settings));
    let _ = write!(out, ",\"env\":{}", env_json(&init.env));
    let _ = write!(
        out,
        ",\"limits\":{{\"slow_ms\":{SLOW_HANDLER_MS},\"log_per_s\":{LOG_LINES_PER_S},\"timer_min_ms\":{TIMER_MIN_MS}}}"
    );
    let _ = write!(out, ",\"dev\":{}}}", init.dev);
    out
}

/// The `frame` text.
pub(crate) fn encode_frame(
    seq: u64,
    applied: u64,
    model_umd1: Option<&str>,
    events: &[&ScriptEvent],
    env: Option<&ScriptEnv>,
) -> String {
    let mut out = format!(
        "{{\"t\":\"frame\",\"seq\":{seq},\"applied\":{applied},\"model\":{}",
        model_umd1.unwrap_or("null")
    );
    out.push_str(",\"events\":[");
    for (i, e) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"name\":{},\"arg\":{},\"node\":{}}}",
            json_str(&e.name),
            value_to_json(&e.arg),
            json_str(&e.node_path)
        );
    }
    out.push(']');
    if let Some(env) = env {
        let _ = write!(out, ",\"env\":{}", env_json(env));
    }
    out.push('}');
    out
}

/// The `stop` text.
pub(crate) const STOP: &str = "{\"t\":\"stop\"}";

/// A type as the JS tables write it.
fn desc(s: &Schema, ty: &Type) -> Json {
    match ty {
        Type::Scalar(sc) => Json::from(sc.name()),
        Type::Key(_) => Json::from("key"),
        Type::Enum(e) => Json::from(vec!["enum", s.enum_def(*e).name.as_str()]),
        Type::Struct(t) => Json::from(vec!["struct", s.type_def(*t).name.as_str()]),
        Type::Option(inner) => Json::Array(vec![Json::from("opt"), desc(s, inner)]),
        Type::List(l) => Json::from(vec!["list", s.type_def(l.elem).name.as_str()]),
    }
}

/// The decoding tables of the parts of a context's model.
fn tables(out: &mut String, parts: &[SchemaPart]) {
    let hash = parts.first().map_or(0, |p| p.schema.hash());
    let mut list = Vec::with_capacity(parts.len());
    for p in parts {
        let s = p.schema;
        let roots: Vec<Json> = s
            .roots()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                Json::Array(vec![
                    Json::from(u32::from(p.root_base) + i as u32),
                    Json::from(r.name.as_str()),
                    desc(s, &r.value_type()),
                ])
            })
            .collect();
        let enums: Map<String, Json> = s
            .enums()
            .iter()
            .map(|e| (e.name.clone(), Json::from(e.variants.clone())))
            .collect();
        let types: Map<String, Json> = s
            .types()
            .iter()
            .map(|t| {
                let fields: Vec<Json> = t
                    .fields
                    .iter()
                    .map(|f| Json::Array(vec![Json::from(f.name.as_str()), desc(s, &f.ty)]))
                    .collect();
                (t.name.clone(), Json::Array(fields))
            })
            .collect();
        let paths: Vec<Json> = s
            .paths()
            .iter()
            .map(|e| {
                Json::Array(vec![
                    Json::from(e.path.as_str()),
                    desc(s, &e.ty),
                    e.list_key.as_deref().map_or(Json::Null, Json::from),
                    Json::from(e.js),
                ])
            })
            .collect();
        let mut o = Map::new();
        o.insert("base".into(), Json::from(p.field_base));
        o.insert("roots".into(), Json::Array(roots));
        o.insert("enums".into(), Json::Object(enums));
        o.insert("types".into(), Json::Object(types));
        o.insert("paths".into(), Json::Array(paths));
        list.push(Json::Object(o));
    }
    let _ = write!(
        out,
        "{{\"hash\":\"{hash:016x}\",\"parts\":{}}}",
        Json::Array(list)
    );
}

/// What a worker sends.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum FromWorker {
    Ready {
        #[serde(default)]
        handlers: Vec<String>,
        #[serde(default)]
        ms: f64,
    },
    Fail {
        error: ErrorInfo,
    },
    Out(OutMsg),
}

/// Where a script failed.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ErrorInfo {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

/// The answer of a worker to a frame (or what a timer or the `ready` handlers produced).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct OutMsg {
    pub n: u64,
    pub ack: Option<u64>,
    pub state: Vec<StateMsg>,
    pub intents: Vec<IntentMsg>,
    pub cmds: Vec<IslandMsg>,
    pub settings: Vec<SettingMsg>,
    pub log: Vec<LogMsg>,
    pub busy_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StateMsg {
    pub p: String,
    #[serde(default)]
    pub v: Json,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IntentMsg {
    pub name: String,
    #[serde(default)]
    pub args: Map<String, Json>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IslandMsg {
    pub island: String,
    pub key: String,
    pub op: String,
    #[serde(default)]
    pub value: Json,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prop: Option<String>,
    #[serde(default)]
    pub on: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SettingMsg {
    pub key: String,
    #[serde(default)]
    pub value: Json,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LogMsg {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    /// What the line is about, for writing it once (`on(sort)|x is not defined`); lines without one are always
    /// written.
    #[serde(default)]
    pub key: Option<String>,
}

impl LogMsg {
    pub(crate) fn to_line(&self) -> LogLine {
        LogLine {
            level: match self.level.as_str() {
                "error" => LogLevel::Error,
                "warn" => LogLevel::Warn,
                _ => LogLevel::Info,
            },
            text: self.text.clone(),
            file: self.file.clone(),
            line: self.line,
        }
    }
}

/// Read a worker's message.
///
/// # Errors
/// Not JSON, or not one of the message kinds.
pub fn decode(text: &str) -> Result<FromWorker, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

impl IslandMsg {
    /// The command, or why it is not one.
    pub(crate) fn to_cmd(&self) -> Result<IslandCmd, String> {
        let op = match self.op.as_str() {
            "text" => IslandOp::Text(match &self.value {
                Json::Object(o) if o.contains_key("loc") => {
                    let key = o.get("loc").and_then(Json::as_str).unwrap_or_default();
                    let args = o
                        .get("args")
                        .and_then(Json::as_array)
                        .map(|a| a.iter().map(|v| Arc::from(json_text(v).as_str())).collect())
                        .unwrap_or_default();
                    Value::Text(Text::Key {
                        key: Arc::from(key),
                        args,
                    })
                }
                v => Value::Text(Text::Raw(Arc::from(json_text(v).as_str()))),
            }),
            "cls" => IslandOp::Cls(
                Arc::from(self.name.as_deref().ok_or("`cls` needs `name`")?),
                self.on.unwrap_or(true),
            ),
            "style" => IslandOp::Style(
                Arc::from(self.prop.as_deref().ok_or("`style` needs `prop`")?),
                json_to_value(&self.value, None),
            ),
            "show" => IslandOp::Show(self.on.unwrap_or(true)),
            other => return Err(format!("unknown island command `{other}`")),
        };
        Ok(IslandCmd {
            island: Arc::from(self.island.as_str()),
            key: Arc::from(self.key.as_str()),
            op,
        })
    }
}

/// A JSON value as text: strings as they are, the rest in JSON form.
fn json_text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// A model value as JSON without the schema (event arguments): enums are indices, keys `"hi:lo"`, structs
/// arrays of their fields, lists arrays of `[key, element]`, a text a string or `{k, a}`.
#[must_use]
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::None => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::I32(n) => Json::from(*n),
        Value::U32(n) => Json::from(*n),
        Value::F32(f) if f.is_finite() => Json::from(f64::from(*f)),
        Value::F32(_) => Json::from(0),
        Value::Str(s) => Json::from(&**s),
        Value::Asset(a) => Json::from(&*a.0),
        Value::Text(Text::Raw(s)) => Json::from(&**s),
        Value::Text(Text::Key { key, args }) => {
            let mut o = Map::new();
            o.insert("k".into(), Json::from(&**key));
            o.insert(
                "a".into(),
                Json::Array(args.iter().map(|a| Json::from(&**a)).collect()),
            );
            Json::Object(o)
        }
        Value::Enum(n) => Json::from(*n),
        Value::Key(k) => Json::from(k.to_js()),
        Value::Struct(fs) => Json::Array(fs.iter().map(value_to_json).collect()),
        Value::List(items) => Json::Array(
            items
                .iter()
                .map(|(k, e)| Json::Array(vec![Json::from(k.to_js()), value_to_json(e)]))
                .collect(),
        ),
    }
}

fn untyped(j: &Json) -> Value {
    match j {
        Json::Null => Value::None,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                Value::I32(i)
            } else if let Some(u) = n.as_u64().and_then(|u| u32::try_from(u).ok()) {
                Value::U32(u)
            } else {
                Value::F32(n.as_f64().unwrap_or(0.0) as f32)
            }
        }
        Json::String(s) => Value::Str(Arc::from(s.as_str())),
        Json::Array(a) => Value::Struct(a.iter().map(untyped).collect()),
        Json::Object(o) => match (o.get("k"), o.get("a")) {
            (Some(Json::String(k)), args) => Value::Text(Text::Key {
                key: Arc::from(k.as_str()),
                args: args
                    .and_then(Json::as_array)
                    .map(|a| a.iter().map(|v| Arc::from(json_text(v).as_str())).collect())
                    .unwrap_or_default(),
            }),
            _ => Value::Struct(o.values().map(untyped).collect()),
        },
    }
}

/// A JSON value from a script as a model value. With the type of the place it goes to, numbers, enum names,
/// keys, texts, structs (objects by field name) and lists take the form of that type; anything that does not
/// fit, or no type, gives the plain reading (whole numbers `I32`, strings `Str`), which the actions module
/// checks and refuses with a message.
#[must_use]
pub fn json_to_value(j: &Json, ty: Option<(&Schema, &Type)>) -> Value {
    let Some((s, ty)) = ty else {
        return untyped(j);
    };
    typed(s, ty, j).unwrap_or_else(|| untyped(j))
}

fn typed(s: &Schema, ty: &Type, j: &Json) -> Option<Value> {
    Some(match (ty, j) {
        (Type::Option(_), Json::Null) => Value::None,
        (Type::Option(inner), j) => typed(s, inner, j)?,
        (Type::Scalar(sc), j) => match (sc, j) {
            (Scalar::Bool, Json::Bool(b)) => Value::Bool(*b),
            (Scalar::I8 | Scalar::I16 | Scalar::I32, Json::Number(n)) => {
                Value::I32(i32::try_from(n.as_i64()?).ok()?)
            }
            (Scalar::U32 | Scalar::Color, Json::Number(n)) => {
                Value::U32(u32::try_from(n.as_u64()?).ok()?)
            }
            (Scalar::F32, Json::Number(n)) => Value::F32(n.as_f64()? as f32),
            (Scalar::Str, Json::String(t)) => Value::Str(Arc::from(t.as_str())),
            (Scalar::Asset, Json::String(t)) => Value::Asset(AssetRef::new(t)),
            (Scalar::Text, Json::String(t)) => Value::Text(Text::raw(t)),
            (Scalar::Text, Json::Object(_)) => untyped(j),
            _ => return None,
        },
        (Type::Key(_), Json::String(t)) => Value::Key(Key::parse_js(t)?),
        (Type::Enum(e), Json::String(name)) => {
            let def = s.enum_def(*e);
            Value::Enum(u16::try_from(def.variants.iter().position(|v| v == name)?).ok()?)
        }
        (Type::Enum(_), Json::Number(n)) => Value::Enum(u16::try_from(n.as_u64()?).ok()?),
        (Type::Struct(t), Json::Object(o)) => {
            let td = s.type_def(*t);
            let mut fields = Vec::with_capacity(td.fields.len());
            for f in &td.fields {
                fields.push(match o.get(&f.name) {
                    Some(v) => typed(s, &f.ty, v)?,
                    None => ui2_model::default_value(s, &f.ty),
                });
            }
            Value::Struct(fields)
        }
        (Type::Struct(t), Json::Array(a)) => {
            let td = s.type_def(*t);
            if a.len() != td.fields.len() {
                return None;
            }
            let mut fields = Vec::with_capacity(a.len());
            for (f, v) in td.fields.iter().zip(a) {
                fields.push(typed(s, &f.ty, v)?);
            }
            Value::Struct(fields)
        }
        (Type::List(l), Json::Array(a)) => {
            let elem = Type::Struct(l.elem);
            let mut items = Vec::with_capacity(a.len());
            for e in a {
                let v = typed(s, &elem, e)?;
                let Value::Struct(fs) = &v else {
                    return None;
                };
                let key = fs.get(l.key_index)?.as_key()?;
                items.push((key, v));
            }
            Value::List(items)
        }
        _ => return None,
    })
}

/// The schema and type of a state path the context writes (`state.<path>` of its own namespace, `ui.<path>`,
/// a mod's `modstate.<id>.<path>`), if it is known.
pub(crate) fn state_type<'p>(
    parts: &'p [SchemaPart],
    ns: &StateNs,
    path: &str,
) -> Option<(&'p Schema, Type)> {
    let full = match (ns, path.strip_prefix("state")) {
        (StateNs::Mod(id), Some(rest)) if rest.is_empty() || rest.starts_with(['.', '[']) => {
            format!("modstate.{id}{rest}")
        }
        _ => path.to_string(),
    };
    parts.iter().find_map(|p| {
        let r = p.schema.resolve(&full).ok()?;
        Some((p.schema, r.ty))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_messages_decode() {
        let ready = decode(r#"{"t":"ready","ctx":"skin","handlers":["sort"],"ms":3.5}"#);
        assert_eq!(
            ready,
            Ok(FromWorker::Ready {
                handlers: vec!["sort".into()],
                ms: 3.5
            })
        );
        let out = decode(
            r#"{"t":"out","n":2,"ack":7,"state":[{"p":"state.a","v":1}],"intents":[{"name":"turn.end","args":{}}],"busy_ms":0.4}"#,
        );
        let Ok(FromWorker::Out(out)) = out else {
            panic!("not an out: {out:?}");
        };
        assert_eq!(
            (out.n, out.ack, out.state.len(), out.intents.len()),
            (2, Some(7), 1, 1)
        );
        assert!(decode(r#"{"t":"nope"}"#).is_err());
        assert!(decode("not json").is_err());
    }

    #[test]
    fn island_commands_decode() {
        let m: IslandMsg = serde_json::from_str(
            r#"{"island":"clock","key":"t","op":"text","value":{"loc":"TXT_KEY_X","args":[3]}}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let cmd = m.to_cmd().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            cmd.op,
            IslandOp::Text(Value::Text(Text::Key {
                key: Arc::from("TXT_KEY_X"),
                args: vec![Arc::from("3")]
            }))
        );
        let m: IslandMsg =
            serde_json::from_str(r#"{"island":"a","key":"b","op":"cls","name":"lit","on":false}"#)
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            m.to_cmd().map(|c| c.op),
            Ok(IslandOp::Cls(Arc::from("lit"), false))
        );
        let m: IslandMsg = serde_json::from_str(r#"{"island":"a","key":"b","op":"spin"}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(m.to_cmd().is_err());
    }

    #[test]
    fn typed_values_follow_the_schema() {
        let s = Schema::v1();
        let ty = |p: &str| s.resolve(p).unwrap_or_else(|e| panic!("{e}")).ty;
        let tab = ty("ui.production.tab");
        assert!(matches!(
            json_to_value(&Json::from("wonders"), Some((s, &tab))),
            Value::Enum(_)
        ));
        let focus = ty("ui.production.focus");
        assert_eq!(
            json_to_value(&Json::from("1:2"), Some((s, &focus))),
            Value::Key(Key::new(1, 2))
        );
        assert_eq!(json_to_value(&Json::Null, Some((s, &focus))), Value::None);
        // A value that does not fit falls back to the plain reading for the actions module to refuse.
        assert_eq!(
            json_to_value(&Json::from("nonsense"), Some((s, &focus))),
            Value::Str(Arc::from("nonsense"))
        );
        assert_eq!(json_to_value(&Json::from(2.5), None), Value::F32(2.5));
        assert_eq!(json_to_value(&Json::from(-3), None), Value::I32(-3));
    }

    #[test]
    fn event_values_encode_plainly() {
        let v = Value::Struct(vec![
            Value::Key(Key::new(0, 5)),
            Value::Text(Text::key("K", &["a"])),
            Value::None,
        ]);
        assert_eq!(
            value_to_json(&v).to_string(),
            r#"["0:5",{"a":["a"],"k":"K"},null]"#
        );
    }
}
