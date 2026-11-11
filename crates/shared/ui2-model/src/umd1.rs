//! UMD1: the JSON text a script worker receives. A delta is `[revision, op, op, ...]`; an op is an array whose
//! first element is the kind:
//!
//! | kind | form |
//! |---|---|
//! | 0 set | `[0, field, [keys], value]` |
//! | 1 insert | `[1, list, [keys], at, key, value]` |
//! | 2 remove | `[2, list, [keys], key]` |
//! | 3 move | `[3, list, [keys], key, to]` |
//! | 4 reset | `[4, root, value]` |
//! | 5 hello | `[5, "<schema hash, 16 hex digits>"]` (first op of a snapshot) |
//!
//! Keys are `"hi:lo"` strings. Values are positional: a struct is an array of its fields, a list an array of
//! `[key, element]`, an enum its index, `None` is `null`, a text is a string or `{"k": key, "a": [args]}`. The
//! path table and the enum names live in `model.gen.js`, which turns the arrays into objects.

use std::fmt::Write as _;

use crate::delta::{FrameDelta, Keys, Op};
use crate::schema::Schema;
use crate::value::{Text, Value};

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn keys(out: &mut String, keys: &Keys) {
    out.push('[');
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "\"{k}\"");
    }
    out.push(']');
}

/// Append the UMD1 form of a value.
pub(crate) fn value(out: &mut String, v: &Value) {
    match v {
        Value::None => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::I32(n) => {
            let _ = write!(out, "{n}");
        }
        Value::U32(n) => {
            let _ = write!(out, "{n}");
        }
        Value::F32(f) => {
            // `Display` for floats never uses an exponent and round-trips; JSON has no NaN or infinity.
            if f.is_finite() {
                let _ = write!(out, "{f}");
            } else {
                out.push('0');
            }
        }
        Value::Str(s) => string(out, s),
        Value::Asset(a) => string(out, &a.0),
        Value::Text(Text::Raw(s)) => string(out, s),
        Value::Text(Text::Key { key, args }) => {
            out.push_str("{\"k\":");
            string(out, key);
            out.push_str(",\"a\":[");
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(out, a);
            }
            out.push_str("]}");
        }
        Value::Enum(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Key(k) => {
            let _ = write!(out, "\"{k}\"");
        }
        Value::Struct(fs) => {
            out.push('[');
            for (i, f) in fs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                value(out, f);
            }
            out.push(']');
        }
        Value::List(items) => {
            out.push('[');
            for (i, (k, e)) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "[\"{k}\",");
                value(out, e);
                out.push(']');
            }
            out.push(']');
        }
    }
}

pub(crate) fn op(out: &mut String, o: &Op) {
    match o {
        Op::Set {
            field,
            keys: ks,
            value: v,
        } => {
            let _ = write!(out, "[0,{},", field.0);
            keys(out, ks);
            out.push(',');
            value(out, v);
        }
        Op::Insert {
            list,
            keys: ks,
            at,
            key,
            value: v,
        } => {
            let _ = write!(out, "[1,{},", list.0);
            keys(out, ks);
            let _ = write!(out, ",{at},\"{key}\",");
            value(out, v);
        }
        Op::Remove {
            list,
            keys: ks,
            key,
        } => {
            let _ = write!(out, "[2,{},", list.0);
            keys(out, ks);
            let _ = write!(out, ",\"{key}\"");
        }
        Op::Move {
            list,
            keys: ks,
            key,
            to,
        } => {
            let _ = write!(out, "[3,{},", list.0);
            keys(out, ks);
            let _ = write!(out, ",\"{key}\",{to}");
        }
        Op::Reset { root, value: v } => {
            let _ = write!(out, "[4,{},", root.0);
            value(out, v);
        }
    }
    out.push(']');
}

impl FrameDelta {
    /// The UMD1 text of this delta. `schema` is the schema the field ids belong to.
    #[must_use]
    pub fn to_umd1(&self, schema: &Schema) -> String {
        debug_assert!(self.ops.iter().all(|o| match o {
            Op::Set { field, .. } => (field.0 as usize) < schema.paths().len(),
            Op::Insert { list, .. } | Op::Remove { list, .. } | Op::Move { list, .. } =>
                (list.0 as usize) < schema.paths().len(),
            Op::Reset { root, .. } => usize::from(root.0) < schema.roots().len(),
        }));
        let mut out = format!("[{}", self.revision);
        for o in &self.ops {
            out.push(',');
            op(&mut out, o);
        }
        out.push(']');
        out
    }
}
