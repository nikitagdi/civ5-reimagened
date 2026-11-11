//! Rust generator: enums and structs with their `Model` impls (the diff of a struct compares fields in place,
//! without building `Value`s).

use std::fmt::Write as _;

use crate::schema::{FieldDef, Scalar, Schema, StructDef, Type, pascal};

const KEYWORDS: [&str; 41] = [
    "as", "break", "const", "continue", "else", "enum", "extern", "false", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static",
    "struct", "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn",
    "abstract", "become", "box", "do", "final", "macro", "override",
];
const KEYWORDS2: [&str; 6] = ["priv", "typeof", "unsized", "virtual", "yield", "try"];

/// A field name as a Rust identifier (`type` becomes `r#type`).
pub fn ident(name: &str) -> String {
    if KEYWORDS.contains(&name) || KEYWORDS2.contains(&name) || name == "gen" {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

fn rust_type(s: &Schema, ty: &Type) -> String {
    match ty {
        Type::Scalar(sc) => match sc {
            Scalar::Bool => "bool".into(),
            Scalar::I8 => "i8".into(),
            Scalar::I16 => "i16".into(),
            Scalar::I32 => "i32".into(),
            Scalar::U32 | Scalar::Color => "u32".into(),
            Scalar::F32 => "f32".into(),
            Scalar::Str => "::std::sync::Arc<str>".into(),
            Scalar::Text => "crate::value::Text".into(),
            Scalar::Asset => "crate::value::AssetRef".into(),
        },
        Type::Key(_) => "crate::value::Key".into(),
        Type::Enum(e) => s.enum_def(*e).name.clone(),
        Type::Struct(t) => s.type_def(*t).name.clone(),
        Type::Option(inner) => format!("Option<{}>", rust_type(s, inner)),
        Type::List(l) => format!("crate::model::KeyedList<{}>", s.type_def(l.elem).name),
    }
}

fn key_expr(f: &FieldDef) -> String {
    let n = ident(&f.name);
    match &f.ty {
        Type::Key(_) => format!("self.{n}"),
        Type::Scalar(Scalar::U32) => format!("crate::value::Key::from_u32(self.{n})"),
        Type::Scalar(Scalar::Str) => format!("crate::value::Key::from_text(&self.{n})"),
        Type::Enum(_) => format!("crate::value::Key::from_enum(self.{n} as u16)"),
        _ => format!("crate::value::Key::from_i32(i32::from(self.{n}))"),
    }
}

fn enum_code(out: &mut String, e: &crate::schema::EnumDef) {
    let name = &e.name;
    let n = e.variants.len();
    let _ = writeln!(
        out,
        "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]"
    );
    let _ = writeln!(out, "#[repr(u16)]");
    let _ = writeln!(out, "pub enum {name} {{");
    for (i, v) in e.variants.iter().enumerate() {
        if i == 0 {
            let _ = writeln!(out, "    #[default]");
        }
        let _ = writeln!(out, "    {} = {i},", pascal(v));
    }
    let _ = writeln!(out, "}}\n");
    let _ = writeln!(out, "impl {name} {{");
    let names: Vec<String> = e.variants.iter().map(|v| format!("{v:?}")).collect();
    let all: Vec<String> = e
        .variants
        .iter()
        .map(|v| format!("{name}::{}", pascal(v)))
        .collect();
    let _ = writeln!(
        out,
        "    /// The names, indexed by the discriminant.\n    pub const NAMES: [&'static str; {n}] = [{}];\n",
        names.join(", ")
    );
    let _ = writeln!(
        out,
        "    /// Every variant, in declaration order.\n    pub const ALL: &'static [{name}] = &[{}];\n",
        all.join(", ")
    );
    let _ = writeln!(
        out,
        "    /// The schema name of the variant (the same as [`name`](Self::name)).\n    #[must_use]\n    pub fn as_str(self) -> &'static str {{\n        Self::NAMES[self as usize]\n    }}\n"
    );
    let _ = writeln!(out, "    /// The variant with this discriminant.");
    let _ = writeln!(
        out,
        "    #[must_use]\n    pub const fn from_u16(v: u16) -> Option<{name}> {{\n        match v {{"
    );
    for (i, v) in e.variants.iter().enumerate() {
        let _ = writeln!(out, "            {i} => Some({name}::{}),", pascal(v));
    }
    let _ = writeln!(out, "            _ => None,\n        }}\n    }}\n");
    let _ = writeln!(out, "    /// The schema name of the variant.");
    let _ = writeln!(
        out,
        "    #[must_use]\n    pub fn name(self) -> &'static str {{\n        Self::NAMES[self as usize]\n    }}\n"
    );
    let _ = writeln!(out, "    /// The variant with this schema name.");
    let _ = writeln!(
        out,
        "    #[must_use]\n    pub fn from_name(s: &str) -> Option<{name}> {{\n        match s {{"
    );
    for v in &e.variants {
        let _ = writeln!(out, "            {v:?} => Some({name}::{}),", pascal(v));
    }
    let _ = writeln!(out, "            _ => None,\n        }}\n    }}\n}}\n");
    let _ = writeln!(
        out,
        "impl crate::model::Model for {name} {{
    fn diff(&self, old: &Self, at: &mut crate::delta::Cursor, out: &mut crate::delta::Ops) {{
        if self != old {{
            out.push(crate::delta::Op::Set {{
                field: at.field,
                keys: at.keys,
                value: crate::value::Value::Enum(*self as u16),
            }});
        }}
    }}

    fn to_value(&self) -> crate::value::Value {{
        crate::value::Value::Enum(*self as u16)
    }}

    fn from_value(v: &crate::value::Value) -> Result<Self, crate::value::ValueError> {{
        match v {{
            crate::value::Value::Enum(n) => {name}::from_u16(*n).ok_or(crate::value::ValueError::EnumIndex {{
                value: *n,
                count: {n},
            }}),
            other => Err(crate::value::ValueError::mismatch(\"enum\", other)),
        }}
    }}
}}\n"
    );
}

fn struct_code(out: &mut String, s: &Schema, td: &StructDef) {
    let name = &td.name;
    let _ = writeln!(out, "#[derive(Clone, Debug, Default, PartialEq)]");
    let _ = writeln!(out, "pub struct {name} {{");
    for f in &td.fields {
        let _ = writeln!(out, "    pub {}: {},", ident(&f.name), rust_type(s, &f.ty));
    }
    let _ = writeln!(out, "}}\n");
    let _ = writeln!(out, "impl crate::model::Model for {name} {{");
    let _ = writeln!(
        out,
        "    fn diff(&self, old: &Self, at: &mut crate::delta::Cursor, out: &mut crate::delta::Ops) {{\n        let base = at.field;"
    );
    for (i, f) in td.fields.iter().enumerate() {
        let n = ident(&f.name);
        let _ = writeln!(
            out,
            "        at.field = base.child({});\n        crate::model::Model::diff(&self.{n}, &old.{n}, at, out);",
            td.offsets[i]
        );
    }
    let _ = writeln!(out, "        at.field = base;\n    }}\n");
    let _ = writeln!(
        out,
        "    fn to_value(&self) -> crate::value::Value {{\n        crate::value::Value::Struct(vec!["
    );
    for f in &td.fields {
        let _ = writeln!(
            out,
            "            crate::model::Model::to_value(&self.{}),",
            ident(&f.name)
        );
    }
    let _ = writeln!(out, "        ])\n    }}\n");
    let _ = writeln!(
        out,
        "    fn from_value(v: &crate::value::Value) -> Result<Self, crate::value::ValueError> {{\n        let f = v.as_struct({})?;\n        Ok({name} {{",
        td.fields.len()
    );
    for (i, f) in td.fields.iter().enumerate() {
        let _ = writeln!(
            out,
            "            {}: crate::model::Model::from_value(&f[{i}])?,",
            ident(&f.name)
        );
    }
    let _ = writeln!(out, "        }})\n    }}\n}}\n");
    if let Some(ki) = td.key_index {
        let _ = writeln!(
            out,
            "impl crate::model::Keyed for {name} {{\n    fn key(&self) -> crate::value::Key {{\n        {}\n    }}\n}}\n",
            key_expr(&td.fields[ki])
        );
    }
}

/// The Rust type that holds a whole root.
fn root_rust_type(s: &Schema, r: &crate::schema::Root) -> String {
    rust_type(s, &r.value_type())
}

/// `impl Root` for the type of every root, and the dispatch from a root name to a typed write.
fn roots_code(out: &mut String, s: &Schema) {
    for r in s.roots() {
        let _ = writeln!(
            out,
            "impl crate::model::Root for {} {{\n    const NAME: &'static str = {:?};\n}}\n",
            root_rust_type(s, r),
            r.name
        );
    }
    let _ = writeln!(
        out,
        "/// Write a root, given by name as a dynamic value, through a writer: the typed form of `Value`-based sources\n/// (samples, remote frames). `None` if the schema has no root of that name.\npub fn put_root(\n    w: &mut crate::store::ModelWriter<'_>,\n    name: &str,\n    v: &crate::value::Value,\n) -> Option<Result<(), crate::model::PutError>> {{\n    Some(match name {{"
    );
    for r in s.roots() {
        let _ = writeln!(
            out,
            "        {:?} => crate::model::put_value::<{}>(w, v),",
            r.name,
            root_rust_type(s, r)
        );
    }
    let _ = writeln!(out, "        _ => return None,\n    }})\n}}");
}

/// The text of `src/gen/model.rs`.
pub fn generate(s: &Schema) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask ui gen-model` from `schema/ui-model.toml`. Do not edit.\n//! Generated model types: one struct per root and type, one enum per enum.\n"
    );
    let _ = writeln!(
        out,
        "/// Schema version this file was generated from.\npub const SCHEMA_VERSION: (u16, u16) = ({}, {});\n/// Hash of the canonical `schema.gen.json` this file was generated from.\npub const SCHEMA_HASH: u64 = 0x{:016x};\n",
        s.version().major,
        s.version().minor,
        s.hash()
    );
    for e in s.enums() {
        enum_code(&mut out, e);
    }
    for t in s.types() {
        struct_code(&mut out, s, t);
    }
    roots_code(&mut out, s);
    out
}
