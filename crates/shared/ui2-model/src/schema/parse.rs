//! Schema source (TOML) to a draft: names collected, types resolved, rules of the language checked. The layout
//! (offsets, path table) is computed by the caller.

use std::collections::{HashMap, HashSet};

use toml::Spanned;
use toml::de::{DeString, DeTable, DeValue};

use super::SchemaError;
use super::types::{
    EnumDef, EnumId, FieldDef, IntentDef, ListType, Scalar, StructDef, Type, TypeId, Version,
};

/// Roots that belong to the dynamic namespaces (state, mods) and cannot be declared as data roots.
const RESERVED_ROOTS: [&str; 3] = ["state", "mods", "modstate"];

/// Type names that would clash with the prelude names used by generated code.
const RESERVED_TYPES: [&str; 12] = [
    "Option", "Vec", "Some", "None", "Box", "String", "Result", "Default", "Clone", "Self", "Ok",
    "Err",
];

/// Names that cannot be written even as raw identifiers.
const NO_RAW: [&str; 3] = ["self", "crate", "super"];

/// Maps byte offsets of the source to 1-based line and column.
#[derive(Clone, Debug)]
pub(crate) struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { starts }
    }

    pub(crate) fn err(&self, offset: usize, msg: impl Into<String>) -> SchemaError {
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        SchemaError::Parse {
            line: line + 1,
            col: offset - self.starts[line] + 1,
            msg: msg.into(),
        }
    }
}

/// Source offsets of a struct definition, for errors found after resolution.
#[derive(Clone, Debug, Default)]
pub(crate) struct StructPos {
    pub at: usize,
    pub fields: Vec<usize>,
}

/// A parsed and resolved schema without layout.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub version: Version,
    pub enums: Vec<EnumDef>,
    /// Named types first (file order), then the root structs.
    pub types: Vec<StructDef>,
    pub pos: Vec<StructPos>,
    /// `(root name, struct)` in file order.
    pub roots: Vec<(String, TypeId)>,
    pub intents: Vec<IntentDef>,
    pub lines: LineIndex,
}

/// A field type before names are resolved.
#[derive(Clone, Debug)]
enum RawTy {
    Scalar(Scalar),
    Key(String),
    Named(String),
    Option(Box<RawTy>),
    List { elem: String, key: String },
}

struct RawField {
    name: String,
    ty: RawTy,
    js: bool,
    pos: usize,
    /// Offset of the type (the value), for errors about it.
    vpos: usize,
}

struct RawStruct {
    name: String,
    fields: Vec<RawField>,
    pos: usize,
    is_root: bool,
}

type Entry<'a, 'i> = (&'a Spanned<DeString<'i>>, &'a Spanned<DeValue<'i>>);

/// Table entries in file order (the table itself is sorted by key).
fn ordered<'a, 'i>(t: &'a DeTable<'i>) -> Vec<Entry<'a, 'i>> {
    let mut v: Vec<Entry<'a, 'i>> = t.iter().collect();
    v.sort_by_key(|(k, _)| k.span().start);
    v
}

fn is_snake(s: &str) -> bool {
    let mut it = s.bytes();
    matches!(it.next(), Some(b'a'..=b'z'))
        && it.all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn is_camel(s: &str) -> bool {
    let mut it = s.bytes();
    matches!(it.next(), Some(b'A'..=b'Z')) && it.all(|c| c.is_ascii_alphanumeric())
}

/// `great_people` to `GreatPeople`.
pub(crate) fn pascal(s: &str) -> String {
    let mut out = String::new();
    for part in s.split('_').filter(|p| !p.is_empty()) {
        let mut cs = part.chars();
        if let Some(c) = cs.next() {
            out.extend(c.to_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

fn kind_name(v: &DeValue<'_>) -> &'static str {
    v.type_str()
}

/// Parse the type of a field written as a string: `i32`, `str?`, `key<unit>`, `Plot`.
fn parse_type_str(s: &str) -> Result<RawTy, String> {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('?') {
        let t = parse_type_str(inner)?;
        if matches!(t, RawTy::Option(_)) {
            return Err(format!("`{s}`: an option of an option is not allowed"));
        }
        return Ok(RawTy::Option(Box::new(t)));
    }
    if let Some(rest) = s.strip_prefix("key<") {
        let Some(ent) = rest.strip_suffix('>') else {
            return Err(format!("`{s}`: malformed key type, expected `key<entity>`"));
        };
        if !is_snake(ent) {
            return Err(format!(
                "`{s}`: the entity of `key<...>` must be a lower-case name"
            ));
        }
        return Ok(RawTy::Key(ent.to_string()));
    }
    if let Some(sc) = Scalar::from_name(s) {
        return Ok(RawTy::Scalar(sc));
    }
    if !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return Ok(RawTy::Named(s.to_string()));
    }
    Err(format!("`{s}`: invalid type syntax"))
}

/// Parse a field value: a type string, or a table (`{ list, key }`, `{ type, js }`).
fn parse_field(
    lines: &LineIndex,
    name: &Spanned<DeString<'_>>,
    value: &Spanned<DeValue<'_>>,
) -> Result<RawField, SchemaError> {
    let fname = name.get_ref().to_string();
    let pos = name.span().start;
    let vpos = value.span().start;
    let mut js = false;
    let ty = match value.get_ref() {
        DeValue::String(s) => parse_type_str(s).map_err(|m| lines.err(vpos, m))?,
        DeValue::Table(t) => {
            let mut ty: Option<RawTy> = None;
            let mut list: Option<(String, usize)> = None;
            let mut key: Option<String> = None;
            for (k, v) in ordered(t) {
                let kpos = k.span().start;
                match (k.get_ref().as_ref(), v.get_ref()) {
                    ("type", DeValue::String(s)) => {
                        ty = Some(parse_type_str(s).map_err(|m| lines.err(v.span().start, m))?);
                    }
                    ("list", DeValue::String(s)) => list = Some((s.to_string(), v.span().start)),
                    ("key", DeValue::String(s)) => key = Some(s.to_string()),
                    ("js", DeValue::Boolean(b)) => js = *b,
                    ("type" | "list" | "key" | "js", other) => {
                        return Err(lines.err(
                            v.span().start,
                            format!(
                                "field `{fname}`: attribute `{}` has the wrong type ({})",
                                k.get_ref(),
                                kind_name(other)
                            ),
                        ));
                    }
                    (other, _) => {
                        return Err(lines.err(
                            kpos,
                            format!(
                                "field `{fname}`: unknown attribute `{other}` (expected `type`, `list`, `key`, `js`); \
                                 an inline struct is not allowed, declare it with `[type.Name]`"
                            ),
                        ));
                    }
                }
            }
            match (ty, list, key) {
                (Some(t), None, None) => t,
                (None, Some((elem, _)), Some(key)) => RawTy::List { elem, key },
                (None, Some((_, p)), None) => {
                    return Err(lines.err(
                        p,
                        format!("field `{fname}`: a list needs `key = \"field\"`"),
                    ));
                }
                (None, None, Some(_)) => {
                    return Err(lines.err(vpos, format!("field `{fname}`: `key` without `list`")));
                }
                (Some(_), _, _) => {
                    return Err(lines.err(
                        vpos,
                        format!("field `{fname}`: `type` cannot be combined with `list`/`key`"),
                    ));
                }
                (None, None, None) => {
                    return Err(lines.err(
                        vpos,
                        format!("field `{fname}`: expected `type = \"...\"` or `list = \"...\"`"),
                    ));
                }
            }
        }
        other => {
            return Err(lines.err(
                vpos,
                format!(
                    "field `{fname}`: expected a type string or a table, found {}",
                    kind_name(other)
                ),
            ));
        }
    };
    Ok(RawField {
        name: fname,
        ty,
        js,
        pos,
        vpos,
    })
}

fn check_field_name(lines: &LineIndex, name: &str, pos: usize) -> Result<(), SchemaError> {
    if name.starts_with('_') {
        return Err(lines.err(
            pos,
            format!("field `{name}`: names starting with `_` are reserved"),
        ));
    }
    if !is_snake(name) {
        return Err(lines.err(
            pos,
            format!("field `{name}`: names are lower-case snake case (`a-z`, `0-9`, `_`)"),
        ));
    }
    if NO_RAW.contains(&name) {
        return Err(lines.err(pos, format!("field `{name}`: reserved word")));
    }
    Ok(())
}

/// A struct body: every entry is a field.
fn parse_body(
    lines: &LineIndex,
    owner: &str,
    t: &DeTable<'_>,
) -> Result<Vec<RawField>, SchemaError> {
    let mut fields: Vec<RawField> = Vec::new();
    for (k, v) in ordered(t) {
        let f = parse_field(lines, k, v)?;
        check_field_name(lines, &f.name, f.pos)?;
        if fields.iter().any(|g| g.name == f.name) {
            return Err(lines.err(f.pos, format!("duplicate field `{}` in `{owner}`", f.name)));
        }
        fields.push(f);
    }
    if fields.is_empty() {
        return Err(lines.err(t_pos(t), format!("`{owner}` has no fields")));
    }
    Ok(fields)
}

fn t_pos(t: &DeTable<'_>) -> usize {
    t.iter().map(|(k, _)| k.span().start).min().unwrap_or(0)
}

/// Parse and resolve a schema source.
pub(crate) fn parse_draft(src: &str) -> Result<Draft, SchemaError> {
    let lines = LineIndex::new(src);
    let doc = DeTable::parse(src).map_err(|e| {
        let at = e.span().map_or(0, |r| r.start);
        lines.err(at, format!("invalid TOML: {}", e.message()))
    })?;
    let root = doc.get_ref();

    let mut version: Option<Version> = None;
    let mut enum_tbl: Option<&DeTable<'_>> = None;
    let mut type_tbl: Option<&DeTable<'_>> = None;
    let mut model_tbl: Option<&DeTable<'_>> = None;
    let mut intent_tbl: Option<&DeTable<'_>> = None;

    for (k, v) in ordered(root) {
        let kpos = k.span().start;
        let table = |what: &str| -> Result<&DeTable<'_>, SchemaError> {
            match v.get_ref() {
                DeValue::Table(t) => Ok(t),
                other => Err(lines.err(
                    v.span().start,
                    format!("`{what}` must be a table, found {}", kind_name(other)),
                )),
            }
        };
        match k.get_ref().as_ref() {
            "version" => {
                let DeValue::String(s) = v.get_ref() else {
                    return Err(
                        lines.err(v.span().start, "`version` must be a string like \"1.0\"")
                    );
                };
                version = Some(Version::parse(s).ok_or_else(|| {
                    lines.err(
                        v.span().start,
                        format!("`version` must be \"MAJOR.MINOR\", found \"{s}\""),
                    )
                })?);
            }
            "enum" => enum_tbl = Some(table("enum")?),
            "type" => type_tbl = Some(table("type")?),
            "model" => model_tbl = Some(table("model")?),
            "intent" => intent_tbl = Some(table("intent")?),
            other => {
                return Err(lines.err(
                    kpos,
                    format!(
                        "unknown top-level key `{other}` (expected `version`, `enum`, `type`, `model`, `intent`)"
                    ),
                ));
            }
        }
    }
    let version = version.ok_or_else(|| lines.err(0, "missing `version = \"MAJOR.MINOR\"`"))?;

    // Names first, so that definitions can refer to each other in any order.
    let mut enums: Vec<EnumDef> = Vec::new();
    let mut enum_ids: HashMap<String, EnumId> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    if let Some(t) = enum_tbl {
        for (k, v) in ordered(t) {
            let name = k.get_ref().to_string();
            let pos = k.span().start;
            check_type_name(&lines, "enum", &name, pos)?;
            if !taken.insert(name.clone()) {
                return Err(lines.err(pos, format!("duplicate type name `{name}`")));
            }
            let DeValue::Array(arr) = v.get_ref() else {
                return Err(lines.err(
                    v.span().start,
                    format!("enum `{name}`: expected an array of names"),
                ));
            };
            let mut variants: Vec<String> = Vec::new();
            let mut pascals: HashSet<String> = HashSet::new();
            for item in arr.iter() {
                let ipos = item.span().start;
                let DeValue::String(s) = item.get_ref() else {
                    return Err(lines.err(ipos, format!("enum `{name}`: variants are strings")));
                };
                if !is_snake(s) {
                    return Err(lines.err(
                        ipos,
                        format!("enum `{name}`: variant `{s}` must be lower-case snake case"),
                    ));
                }
                if NO_RAW.contains(&s.as_ref()) {
                    return Err(lines.err(
                        ipos,
                        format!("enum `{name}`: variant `{s}` is a reserved word"),
                    ));
                }
                if variants.iter().any(|x| x == s.as_ref()) {
                    return Err(lines.err(ipos, format!("enum `{name}`: duplicate variant `{s}`")));
                }
                if !pascals.insert(pascal(s)) {
                    return Err(lines.err(
                        ipos,
                        format!("enum `{name}`: variant `{s}` clashes with another after the Rust name mapping"),
                    ));
                }
                variants.push(s.to_string());
            }
            if variants.is_empty() {
                return Err(lines.err(v.span().start, format!("enum `{name}` has no variants")));
            }
            if variants.len() > usize::from(u16::MAX) {
                return Err(lines.err(
                    v.span().start,
                    format!("enum `{name}` has too many variants"),
                ));
            }
            enum_ids.insert(name.clone(), EnumId(enums.len() as u32));
            enums.push(EnumDef { name, variants });
        }
    }

    let mut raws: Vec<RawStruct> = Vec::new();
    if let Some(t) = type_tbl {
        for (k, v) in ordered(t) {
            let name = k.get_ref().to_string();
            let pos = k.span().start;
            check_type_name(&lines, "type", &name, pos)?;
            if !taken.insert(name.clone()) {
                return Err(lines.err(pos, format!("duplicate type name `{name}`")));
            }
            let DeValue::Table(body) = v.get_ref() else {
                return Err(lines.err(
                    v.span().start,
                    format!("type `{name}`: expected a table of fields"),
                ));
            };
            let fields = parse_body(&lines, &name, body)?;
            raws.push(RawStruct {
                name,
                fields,
                pos,
                is_root: false,
            });
        }
    }
    let mut root_names: Vec<(String, usize)> = Vec::new();
    if let Some(t) = model_tbl {
        for (k, v) in ordered(t) {
            let name = k.get_ref().to_string();
            let pos = k.span().start;
            if !is_snake(&name) || NO_RAW.contains(&name.as_str()) {
                return Err(lines.err(
                    pos,
                    format!("root `{name}`: names are lower-case snake case"),
                ));
            }
            if RESERVED_ROOTS.contains(&name.as_str()) {
                return Err(lines.err(
                    pos,
                    format!("root `{name}` is reserved for the state and mod namespaces"),
                ));
            }
            if root_names.iter().any(|(n, _)| *n == name) {
                return Err(lines.err(pos, format!("duplicate root `{name}`")));
            }
            let DeValue::Table(body) = v.get_ref() else {
                return Err(lines.err(
                    v.span().start,
                    format!("root `{name}`: expected a table of fields"),
                ));
            };
            let sname = pascal(&name);
            if !taken.insert(sname.clone()) {
                return Err(lines.err(
                    pos,
                    format!("root `{name}`: its struct name `{sname}` clashes with another type"),
                ));
            }
            let fields = parse_body(&lines, &name, body)?;
            raws.push(RawStruct {
                name: sname,
                fields,
                pos,
                is_root: true,
            });
            root_names.push((name, pos));
        }
    }

    let mut type_ids: HashMap<String, TypeId> = HashMap::new();
    for (i, r) in raws.iter().enumerate() {
        type_ids.insert(r.name.clone(), TypeId(i as u32));
    }

    // Resolve field types. List keys need the element structs, so they go second.
    let resolve_plain = |raw: &RawTy, pos: usize| -> Result<Type, SchemaError> {
        fn go(
            lines: &LineIndex,
            enum_ids: &HashMap<String, EnumId>,
            type_ids: &HashMap<String, TypeId>,
            raw: &RawTy,
            pos: usize,
        ) -> Result<Type, SchemaError> {
            Ok(match raw {
                RawTy::Scalar(s) => Type::Scalar(*s),
                RawTy::Key(e) => Type::Key(e.clone()),
                RawTy::Named(n) => {
                    if let Some(e) = enum_ids.get(n) {
                        Type::Enum(*e)
                    } else if let Some(t) = type_ids.get(n) {
                        Type::Struct(*t)
                    } else {
                        return Err(lines.err(pos, format!("unknown type `{n}`")));
                    }
                }
                RawTy::Option(inner) => {
                    Type::Option(Box::new(go(lines, enum_ids, type_ids, inner, pos)?))
                }
                RawTy::List { .. } => unreachable!("lists are resolved separately"),
            })
        }
        go(&lines, &enum_ids, &type_ids, raw, pos)
    };

    let mut types: Vec<StructDef> = Vec::new();
    let mut positions: Vec<StructPos> = Vec::new();
    // Struct of each element type: key field name -> (first list that used it) for the consistency check.
    for r in &raws {
        let mut fields = Vec::new();
        for f in &r.fields {
            let ty = match &f.ty {
                RawTy::List { elem, key } => {
                    let Some(&elem_id) = type_ids.get(elem) else {
                        return Err(lines.err(
                            f.pos,
                            format!("field `{}`: unknown list element type `{elem}`", f.name),
                        ));
                    };
                    let es = &raws[elem_id.0 as usize];
                    if es.is_root {
                        return Err(lines.err(
                            f.pos,
                            format!("field `{}`: a root struct cannot be a list element", f.name),
                        ));
                    }
                    let Some(ki) = es.fields.iter().position(|g| g.name == *key) else {
                        return Err(lines.err(
                            f.pos,
                            format!(
                                "field `{}`: list key `{key}` is not a field of `{elem}`",
                                f.name
                            ),
                        ));
                    };
                    let kty = match &es.fields[ki].ty {
                        RawTy::List { .. } => {
                            return Err(lines.err(
                                f.pos,
                                format!("list key `{key}` of `{elem}` cannot be a list"),
                            ));
                        }
                        other => resolve_plain(other, es.fields[ki].vpos)?,
                    };
                    let ok = match &kty {
                        Type::Key(_) | Type::Enum(_) => true,
                        Type::Scalar(s) => matches!(
                            s,
                            Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::U32 | Scalar::Str
                        ),
                        _ => false,
                    };
                    if !ok {
                        return Err(lines.err(
                            f.pos,
                            format!(
                                "field `{}`: list key `{key}` of `{elem}` must be `key<...>`, an integer, an enum or `str` (not optional)",
                                f.name
                            ),
                        ));
                    }
                    Type::List(ListType {
                        elem: elem_id,
                        key: key.clone(),
                        key_index: ki,
                        key_ty: Box::new(kty),
                    })
                }
                other => resolve_plain(other, f.vpos)?,
            };
            fields.push(FieldDef {
                name: f.name.clone(),
                ty,
                js: f.js,
            });
        }
        types.push(StructDef {
            name: r.name.clone(),
            fields,
            is_root: r.is_root,
            key_index: None,
            offsets: Vec::new(),
            body_len: 0,
        });
        positions.push(StructPos {
            at: r.pos,
            fields: r.fields.iter().map(|f| f.pos).collect(),
        });
    }

    // Element structs: one key field per struct (the generated code has one `Keyed` impl).
    let mut key_of: HashMap<TypeId, (usize, usize)> = HashMap::new();
    for (ti, td) in types.iter().enumerate() {
        for (fi, f) in td.fields.iter().enumerate() {
            if let Type::List(l) = &f.ty {
                match key_of.get(&l.elem) {
                    Some(&(ki, _)) if ki != l.key_index => {
                        let es = &types[l.elem.0 as usize];
                        return Err(lines.err(
                            positions[ti].fields[fi],
                            format!(
                                "struct `{}` is a list element with different keys (`{}` and `{}`)",
                                es.name, es.fields[ki].name, l.key
                            ),
                        ));
                    }
                    _ => {
                        key_of.insert(l.elem, (l.key_index, ti));
                    }
                }
            }
        }
    }
    for (elem, (ki, _)) in key_of {
        types[elem.0 as usize].key_index = Some(ki);
    }

    // Intents.
    let mut intents: Vec<IntentDef> = Vec::new();
    if let Some(t) = intent_tbl {
        for (k, v) in ordered(t) {
            let name = k.get_ref().to_string();
            let pos = k.span().start;
            if name.is_empty() || !name.split('.').all(is_snake) {
                return Err(lines.err(
                    pos,
                    format!("intent `{name}`: names are lower-case segments joined by `.`"),
                ));
            }
            if intents.iter().any(|i| i.name == name) {
                return Err(lines.err(pos, format!("duplicate intent `{name}`")));
            }
            let DeValue::Table(body) = v.get_ref() else {
                return Err(lines.err(
                    v.span().start,
                    format!("intent `{name}`: expected a table of arguments"),
                ));
            };
            let mut args: Vec<FieldDef> = Vec::new();
            for (ak, av) in ordered(body) {
                let f = parse_field(&lines, ak, av)?;
                check_field_name(&lines, &f.name, f.pos)?;
                if args.iter().any(|g| g.name == f.name) {
                    return Err(lines.err(
                        f.pos,
                        format!("duplicate argument `{}` in intent `{name}`", f.name),
                    ));
                }
                if f.js {
                    return Err(lines.err(
                        f.pos,
                        format!("argument `{}`: `js` applies to model fields only", f.name),
                    ));
                }
                if matches!(f.ty, RawTy::List { .. }) {
                    return Err(lines.err(
                        f.pos,
                        format!("argument `{}`: lists are not allowed in intents", f.name),
                    ));
                }
                let ty = resolve_plain(&f.ty, f.vpos)?;
                if ty.struct_id().is_some() {
                    return Err(lines.err(
                        f.pos,
                        format!("argument `{}`: structs are not allowed in intents", f.name),
                    ));
                }
                args.push(FieldDef {
                    name: f.name,
                    ty,
                    js: false,
                });
            }
            intents.push(IntentDef { name, args });
        }
    }

    let roots = root_names
        .iter()
        .map(|(n, _)| {
            let tid = type_ids[&pascal(n)];
            (n.clone(), tid)
        })
        .collect();

    Ok(Draft {
        version,
        enums,
        types,
        pos: positions,
        roots,
        intents,
        lines,
    })
}

fn check_type_name(
    lines: &LineIndex,
    what: &str,
    name: &str,
    pos: usize,
) -> Result<(), SchemaError> {
    if !is_camel(name) {
        return Err(lines.err(
            pos,
            format!("{what} `{name}`: type names are CamelCase (`A-Z`, `a-z`, `0-9`)"),
        ));
    }
    if RESERVED_TYPES.contains(&name) {
        return Err(lines.err(pos, format!("{what} `{name}`: reserved name")));
    }
    Ok(())
}
