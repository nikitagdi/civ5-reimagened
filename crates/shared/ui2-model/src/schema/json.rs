//! The canonical JSON form of a schema (`schema.gen.json`).

use serde::Serialize;

use super::{Schema, Type};

#[derive(Serialize)]
struct RootRow<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    ty: &'a str,
    field: u32,
    owner: &'static str,
}

#[derive(Serialize)]
struct FieldRow<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    js: bool,
}

#[derive(Serialize)]
struct TypeRow<'a> {
    name: &'a str,
    fields: Vec<FieldRow<'a>>,
}

#[derive(Serialize)]
struct EnumRow<'a> {
    name: &'a str,
    variants: &'a [String],
}

#[derive(Serialize)]
struct IntentRow<'a> {
    name: &'a str,
    args: Vec<FieldRow<'a>>,
}

#[derive(Serialize)]
struct PathRow<'a> {
    path: &'a str,
    #[serde(rename = "type")]
    ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_key: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    js: bool,
}

fn line<T: Serialize>(v: &T) -> String {
    // The rows hold strings and numbers only: serialising cannot fail.
    serde_json::to_string(v).unwrap_or_default()
}

fn section<T: Serialize>(out: &mut String, name: &str, rows: &[T], last: bool) {
    out.push_str(&format!("\"{name}\": [\n"));
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&line(r));
        out.push_str(if i + 1 < rows.len() { ",\n" } else { "\n" });
    }
    out.push_str(if last { "]\n" } else { "],\n" });
}

fn field_row<'a>(s: &Schema, f: &'a super::FieldDef) -> FieldRow<'a> {
    FieldRow {
        name: &f.name,
        ty: s.type_name(&f.ty),
        key: match &f.ty {
            Type::List(l) => Some(&l.key),
            _ => None,
        },
        js: f.js,
    }
}

fn body(s: &Schema, with_hash: bool) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!("\"version\": \"{}\",\n", s.version));
    if with_hash {
        out.push_str(&format!("\"hash\": \"{:016x}\",\n", s.hash));
    }
    let roots: Vec<RootRow<'_>> = s
        .roots
        .iter()
        .map(|r| RootRow {
            name: &r.name,
            ty: &s.types[r.ty.0 as usize].name,
            field: r.field.0,
            owner: match r.kind {
                super::RootKind::Data => "source",
                super::RootKind::Ui => "actions",
            },
        })
        .collect();
    section(&mut out, "roots", &roots, false);
    let types: Vec<TypeRow<'_>> = s
        .types
        .iter()
        .map(|t| TypeRow {
            name: &t.name,
            fields: t.fields.iter().map(|f| field_row(s, f)).collect(),
        })
        .collect();
    section(&mut out, "types", &types, false);
    let enums: Vec<EnumRow<'_>> = s
        .enums
        .iter()
        .map(|e| EnumRow {
            name: &e.name,
            variants: &e.variants,
        })
        .collect();
    section(&mut out, "enums", &enums, false);
    let intents: Vec<IntentRow<'_>> = s
        .intents
        .iter()
        .map(|i| IntentRow {
            name: &i.name,
            args: i.args.iter().map(|f| field_row(s, f)).collect(),
        })
        .collect();
    section(&mut out, "intents", &intents, false);
    let paths: Vec<PathRow<'_>> = s
        .paths
        .iter()
        .map(|p| PathRow {
            path: &p.path,
            ty: s.type_name(&p.ty),
            list_key: p.list_key.as_deref(),
            js: p.js,
        })
        .collect();
    section(&mut out, "paths", &paths, true);
    out.push_str("}\n");
    out
}

/// The text that the schema hash covers (no `hash` member).
pub(super) fn canonical(s: &Schema) -> String {
    body(s, false)
}

/// The text of `schema.gen.json`.
pub(super) fn file(s: &Schema) -> String {
    body(s, true)
}
