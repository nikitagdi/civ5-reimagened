//! The schema: the single source of the view model. Parsed from TOML ([`Schema::parse`]); everything else
//! (Rust types, JS, JSON, the path table) is derived from it.

mod json;
mod parse;
mod types;

use std::collections::HashMap;
use std::sync::OnceLock;

use thiserror::Error;

pub use types::{
    EnumDef, EnumId, FieldDef, FieldId, FieldRef, IntentDef, ListType, MAX_LIST_DEPTH, PathEntry,
    Root, RootId, RootKind, Scalar, StructDef, Type, TypeId, Version,
};

use parse::Draft;

/// The v1 schema source, embedded in the crate.
const V1_SOURCE: &str = include_str!("../../schema/ui-model.toml");

/// A schema problem. The text is meant to be shown to the author as is.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SchemaError {
    /// A problem in a schema source, with its 1-based position.
    #[error("schema:{line}:{col}: {msg}")]
    Parse {
        line: usize,
        col: usize,
        msg: String,
    },
    /// A path that does not resolve.
    #[error("path `{path}`: {msg}")]
    Path { path: String, msg: String },
}

/// A parsed, validated schema with its flat path table. Immutable.
#[derive(Clone, Debug)]
pub struct Schema {
    version: Version,
    enums: Vec<EnumDef>,
    types: Vec<StructDef>,
    roots: Vec<Root>,
    intents: Vec<IntentDef>,
    paths: Vec<PathEntry>,
    by_path: HashMap<String, FieldId>,
    hash: u64,
}

impl Schema {
    /// Parse and validate a schema source.
    ///
    /// # Errors
    /// The first problem found, with its line and column.
    pub fn parse(src: &str) -> Result<Schema, SchemaError> {
        let draft = parse::parse_draft(src)?;
        Schema::build(draft)
    }

    /// The v1 schema of the crate (`schema/ui-model.toml`), parsed once.
    ///
    /// # Panics
    /// Never for a released build: the source is checked by the tests.
    #[must_use]
    pub fn v1() -> &'static Schema {
        static V1: OnceLock<Schema> = OnceLock::new();
        V1.get_or_init(|| Schema::parse(V1_SOURCE).expect("the embedded v1 schema is valid"))
    }

    /// The embedded v1 schema source.
    #[must_use]
    pub fn v1_source() -> &'static str {
        V1_SOURCE
    }

    fn build(mut d: Draft) -> Result<Schema, SchemaError> {
        // A root struct is not a type that other structs may hold.
        for (ti, td) in d.types.iter().enumerate() {
            for (fi, f) in td.fields.iter().enumerate() {
                let target = match &f.ty {
                    Type::List(l) => Some(l.elem),
                    other => other.struct_id(),
                };
                if let Some(t) = target
                    && d.types[t.0 as usize].is_root
                {
                    return Err(d.lines.err(
                        d.pos[ti].fields[fi],
                        format!(
                            "field `{}`: the struct of root `{}` cannot be used as a type",
                            f.name, d.types[t.0 as usize].name
                        ),
                    ));
                }
            }
        }

        // Layout: cycles, body lengths, offsets.
        let n = d.types.len();
        let mut state = vec![0u8; n];
        let mut body = vec![0u32; n];
        let mut offsets: Vec<Vec<u32>> = vec![Vec::new(); n];
        for t in 0..n {
            let mut stack = Vec::new();
            layout(&d, t, &mut state, &mut body, &mut offsets, &mut stack)?;
        }
        for (i, td) in d.types.iter_mut().enumerate() {
            td.body_len = body[i];
            td.offsets = std::mem::take(&mut offsets[i]);
        }

        // `js` fields exist only below the `ui` root.
        for (name, tid) in &d.roots {
            if name == "ui" {
                continue;
            }
            let mut seen = vec![false; n];
            let mut todo = vec![tid.0 as usize];
            while let Some(t) = todo.pop() {
                if std::mem::replace(&mut seen[t], true) {
                    continue;
                }
                for (fi, f) in d.types[t].fields.iter().enumerate() {
                    if f.js {
                        return Err(d.lines.err(
                            d.pos[t].fields[fi],
                            format!(
                                "field `{}`: `js = true` is allowed only below the `ui` root (reached from `{name}`)",
                                f.name
                            ),
                        ));
                    }
                    if let Some(c) = child_struct(&f.ty) {
                        todo.push(c.0 as usize);
                    }
                }
            }
        }

        // The flat path table, in the same order the layout assumed.
        let mut roots = Vec::new();
        let mut paths: Vec<PathEntry> = Vec::new();
        for (ri, (name, tid)) in d.roots.iter().enumerate() {
            let id = FieldId(paths.len() as u32);
            let kind = if name == "ui" {
                RootKind::Ui
            } else {
                RootKind::Data
            };
            roots.push(Root {
                name: name.clone(),
                ty: *tid,
                field: id,
                kind,
            });
            paths.push(PathEntry {
                path: name.clone(),
                ty: Type::Struct(*tid),
                root: RootId(ri as u16),
                steps: Vec::new(),
                list_key: None,
                js: false,
                end: 0,
            });
            let mut walker = Walker {
                d: &d,
                paths: &mut paths,
                root: RootId(ri as u16),
            };
            walker.walk(id, name, *tid, &[], 0)?;
            let end = paths.len() as u32;
            paths[id.0 as usize].end = end;
        }

        let by_path = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.path.clone(), FieldId(i as u32)))
            .collect();
        let mut schema = Schema {
            version: d.version,
            enums: d.enums,
            types: d.types,
            roots,
            intents: d.intents,
            paths,
            by_path,
            hash: 0,
        };
        schema.hash = fnv1a(json::canonical(&schema).as_bytes());
        Ok(schema)
    }

    /// `MAJOR.MINOR` of the schema.
    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    /// FNV-1a of the canonical `schema.gen.json` text (without the hash itself).
    #[must_use]
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// The model roots in schema order.
    #[must_use]
    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    /// One root.
    ///
    /// # Panics
    /// If `id` is not from this schema.
    #[must_use]
    pub fn root(&self, id: RootId) -> &Root {
        &self.roots[usize::from(id.0)]
    }

    /// Find a root by name.
    #[must_use]
    pub fn root_by_name(&self, name: &str) -> Option<RootId> {
        self.roots
            .iter()
            .position(|r| r.name == name)
            .map(|i| RootId(i as u16))
    }

    /// All struct types: named types first, then the root structs.
    #[must_use]
    pub fn types(&self) -> &[StructDef] {
        &self.types
    }

    /// One struct type.
    ///
    /// # Panics
    /// If `id` is not from this schema.
    #[must_use]
    pub fn type_def(&self, id: TypeId) -> &StructDef {
        &self.types[id.0 as usize]
    }

    /// All enums.
    #[must_use]
    pub fn enums(&self) -> &[EnumDef] {
        &self.enums
    }

    /// One enum.
    ///
    /// # Panics
    /// If `id` is not from this schema.
    #[must_use]
    pub fn enum_def(&self, id: EnumId) -> &EnumDef {
        &self.enums[id.0 as usize]
    }

    /// All intents.
    #[must_use]
    pub fn intents(&self) -> &[IntentDef] {
        &self.intents
    }

    /// The flat path table; the index is the [`FieldId`].
    #[must_use]
    pub fn paths(&self) -> &[PathEntry] {
        &self.paths
    }

    /// One row of the path table.
    ///
    /// # Panics
    /// If `id` is not from this schema.
    #[must_use]
    pub fn path(&self, id: FieldId) -> &PathEntry {
        &self.paths[id.0 as usize]
    }

    /// Resolve a path written by an author (`top.gold`, `top.resources[].name`).
    ///
    /// # Errors
    /// The path is not in the table; the message suggests the closest known path when there is one.
    pub fn resolve(&self, path: &str) -> Result<FieldRef, SchemaError> {
        match self.by_path.get(path) {
            Some(&id) => Ok(FieldRef {
                id,
                ty: self.paths[id.0 as usize].ty.clone(),
            }),
            None => Err(SchemaError::Path {
                path: path.to_string(),
                msg: self.why_unknown(path),
            }),
        }
    }

    fn why_unknown(&self, path: &str) -> String {
        let root = path.split(['.', '[']).next().unwrap_or("");
        if self.root_by_name(root).is_none() {
            return format!("unknown root `{root}`");
        }
        // The longest known prefix tells where it went wrong.
        let mut parent = path;
        while let Some(i) = parent.rfind('.') {
            parent = &parent[..i];
            if let Some(&id) = self.by_path.get(parent) {
                let bad = path[parent.len() + 1..].split('.').next().unwrap_or("");
                if matches!(self.paths[id.0 as usize].ty, Type::List(_)) {
                    return format!(
                        "`{parent}` is a list: write `{parent}[].{bad}` for a field of its elements"
                    );
                }
                return format!("`{parent}` has no field `{bad}`");
            }
        }
        "unknown path".to_string()
    }

    /// The type written the way the schema source writes it (`i32?`, `key<unit>`, `list<Resource>`).
    #[must_use]
    pub fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Scalar(s) => s.name().to_string(),
            Type::Key(e) => format!("key<{e}>"),
            Type::Enum(e) => self.enums[e.0 as usize].name.clone(),
            Type::Struct(t) => self.types[t.0 as usize].name.clone(),
            Type::Option(inner) => format!("{}?", self.type_name(inner)),
            Type::List(l) => format!("list<{}>", self.types[l.elem.0 as usize].name),
        }
    }

    /// The canonical JSON file for tools (`schema.gen.json`): version, hash, roots, types, enums, intents and
    /// the path table. Deterministic.
    #[must_use]
    pub fn to_json(&self) -> String {
        json::file(self)
    }
}

/// FNV-1a, 64 bit.
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The struct below a node: the field itself or a list element.
fn child_struct(ty: &Type) -> Option<TypeId> {
    match ty {
        Type::List(l) => Some(l.elem),
        other => other.struct_id(),
    }
}

fn layout(
    d: &Draft,
    t: usize,
    state: &mut [u8],
    body: &mut [u32],
    offsets: &mut [Vec<u32>],
    stack: &mut Vec<usize>,
) -> Result<(), SchemaError> {
    match state[t] {
        2 => return Ok(()),
        1 => {
            let from = stack.iter().position(|&x| x == t).unwrap_or(0);
            let chain: Vec<&str> = stack[from..]
                .iter()
                .chain(std::iter::once(&t))
                .map(|&i| d.types[i].name.as_str())
                .collect();
            return Err(d.lines.err(
                d.pos[t].at,
                format!(
                    "type `{}` contains itself: {}",
                    d.types[t].name,
                    chain.join(" -> ")
                ),
            ));
        }
        _ => {}
    }
    state[t] = 1;
    stack.push(t);
    let mut offs = Vec::with_capacity(d.types[t].fields.len());
    let mut total = 0u32;
    for f in &d.types[t].fields {
        offs.push(total);
        total += 1;
        if let Some(c) = child_struct(&f.ty) {
            let c = c.0 as usize;
            layout(d, c, state, body, offsets, stack)?;
            total += body[c];
        }
    }
    stack.pop();
    state[t] = 2;
    body[t] = total;
    offsets[t] = offs;
    Ok(())
}

struct Walker<'a> {
    d: &'a Draft,
    paths: &'a mut Vec<PathEntry>,
    root: RootId,
}

impl Walker<'_> {
    /// Add the fields of struct `t` below the node `node`, whose path text is `prefix`.
    fn walk(
        &mut self,
        node: FieldId,
        prefix: &str,
        t: TypeId,
        steps: &[u16],
        depth: usize,
    ) -> Result<(), SchemaError> {
        let td = &self.d.types[t.0 as usize];
        for (j, f) in td.fields.iter().enumerate() {
            let id = FieldId(self.paths.len() as u32);
            debug_assert_eq!(id, node.child(td.offsets[j]));
            let path = format!("{prefix}.{}", f.name);
            let mut st = steps.to_vec();
            st.push(j as u16);
            let list_key = match &f.ty {
                Type::List(l) => Some(l.key.clone()),
                _ => None,
            };
            self.paths.push(PathEntry {
                path: path.clone(),
                ty: f.ty.clone(),
                root: self.root,
                steps: st.clone(),
                list_key,
                js: f.js,
                end: 0,
            });
            match &f.ty {
                Type::List(l) => {
                    if depth + 1 > MAX_LIST_DEPTH {
                        return Err(self.d.lines.err(
                            self.d.pos[t.0 as usize].fields[j],
                            format!(
                                "field `{path}`: lists are nested deeper than {MAX_LIST_DEPTH}"
                            ),
                        ));
                    }
                    self.walk(id, &format!("{path}[]"), l.elem, &st, depth + 1)?;
                }
                other => {
                    if let Some(c) = other.struct_id() {
                        self.walk(id, &path, c, &st, depth)?;
                    }
                }
            }
            let end = self.paths.len() as u32;
            self.paths[id.0 as usize].end = end;
        }
        Ok(())
    }
}
