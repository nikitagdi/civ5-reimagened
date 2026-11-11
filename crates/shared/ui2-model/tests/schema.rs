//! Schema parser: the reference schema, path resolution, layout, and the error texts.

use ui2_model::{FieldId, RootKind, RootShape, Scalar, Schema, SchemaError, Type};

const REFERENCE: &str = r#"
version = "1.0"
[enum]
Yield = ["food", "production", "gold", "science", "culture", "faith", "tourism", "happiness"]
UnitActionKind = ["move", "fortify"]
[type]
Plot = { x = "i16", y = "i16" }
[type.CityBanner]
key  = "key<city>"
name = "str"
production = "BannerProd?"
[type.BannerProd]
item = "key<item>"
turns = "i16?"
[type.Resource]
id = "key<item>"
name = "text"
available = "i32"
[model.top]
gold = "i32"
tourism = "i32?"
resources = { list = "Resource", key = "id" }
[model.ui]
pos = "Plot"
tab = { type = "Yield", js = true }
cities = { list = "CityBanner", key = "key" }
[intent."unit.action"]
unit = "key<unit>"
action = "UnitActionKind"
"#;

fn parse_err(src: &str) -> String {
    match Schema::parse(src) {
        Ok(_) => panic!("expected an error for:\n{src}"),
        Err(e) => e.to_string(),
    }
}

fn with_header(body: &str) -> String {
    format!("version = \"1.0\"\n{body}")
}

#[test]
fn reference_schema_parses() {
    let s = Schema::parse(REFERENCE).expect("reference parses");
    assert_eq!(s.version().to_string(), "1.0");
    assert_eq!(s.enums().len(), 2);
    assert_eq!(s.enums()[0].name, "Yield");
    assert_eq!(s.enums()[0].variants[2], "gold");
    assert_eq!(s.roots().len(), 2);
    assert_eq!(s.roots()[0].name, "top");
    assert_eq!(s.roots()[0].kind, RootKind::Data);
    assert_eq!(s.roots()[1].kind, RootKind::Ui);
    assert_eq!(s.intents().len(), 1);
    assert_eq!(s.intents()[0].name, "unit.action");
    assert_eq!(s.intents()[0].args[0].name, "unit");
    // Types keep the order of the file, root structs come last.
    let names: Vec<&str> = s.types().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["Plot", "CityBanner", "BannerProd", "Resource", "Top", "Ui"]
    );
    // Field order is the file order, not alphabetical.
    let city: Vec<&str> = s.types()[1]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(city, ["key", "name", "production"]);
}

#[test]
fn resolve_paths() {
    let s = Schema::parse(REFERENCE).unwrap();
    let gold = s.resolve("top.gold").unwrap();
    assert_eq!(gold.ty, Type::Scalar(Scalar::I32));
    let tourism = s.resolve("top.tourism").unwrap();
    assert_eq!(
        tourism.ty,
        Type::Option(Box::new(Type::Scalar(Scalar::I32)))
    );
    let list = s.resolve("top.resources").unwrap();
    assert!(matches!(list.ty, Type::List(_)));
    assert_eq!(s.path(list.id).list_key.as_deref(), Some("id"));
    let el = s.resolve("top.resources[].name").unwrap();
    assert_eq!(el.ty, Type::Scalar(Scalar::Text));
    // An optional struct is walked through without a marker.
    let turns = s.resolve("ui.cities[].production.turns").unwrap();
    assert!(matches!(turns.ty, Type::Option(_)));
    assert!(s.resolve("ui.pos.x").is_ok());
    // js is recorded.
    let tab = s.resolve("ui.tab").unwrap();
    assert!(s.path(tab.id).js);
    assert!(!s.path(gold.id).js);
}

#[test]
fn path_table_layout_is_a_preorder() {
    let s = Schema::parse(REFERENCE).unwrap();
    for (i, p) in s.paths().iter().enumerate() {
        // Descendants are the ids right after the node, up to `end`.
        assert!(p.end as usize > i && p.end as usize <= s.paths().len());
        for d in (i + 1)..p.end as usize {
            let child = &s.paths()[d].path;
            assert!(
                child.starts_with(&p.path)
                    && matches!(child.as_bytes().get(p.path.len()), Some(b'.' | b'[')),
                "{child} is not below {}",
                p.path
            );
        }
        let after = s.paths().get(p.end as usize);
        if let Some(a) = after {
            assert!(!a.path.starts_with(&format!("{}.", p.path)));
        }
    }
    // Struct offsets are what the walk produced.
    for (ri, r) in s.roots().iter().enumerate() {
        let td = s.type_def(r.ty);
        for (j, f) in td.fields.iter().enumerate() {
            let id = r.field.child(td.offsets[j]);
            assert_eq!(
                s.path(id).path,
                format!("{}.{}", s.roots()[ri].name, f.name)
            );
        }
    }
    assert_eq!(s.root(s.root_by_name("top").unwrap()).field, FieldId(0));
}

#[test]
fn json_is_deterministic_and_has_the_hash() {
    let a = Schema::parse(REFERENCE).unwrap();
    let b = Schema::parse(REFERENCE).unwrap();
    assert_eq!(a.to_json(), b.to_json());
    assert_eq!(a.hash(), b.hash());
    let v: serde_json::Value = serde_json::from_str(&a.to_json()).expect("valid JSON");
    assert_eq!(v["version"], "1.0");
    assert_eq!(v["hash"], format!("{:016x}", a.hash()));
    assert_eq!(v["roots"][0]["name"], "top");
    assert_eq!(v["paths"][0]["path"], "top");
    assert_eq!(v["intents"][0]["name"], "unit.action");
    // Another schema, another hash.
    let c = Schema::parse(&REFERENCE.replace("gold = \"i32\"", "gold = \"u32\"")).unwrap();
    assert_ne!(a.hash(), c.hash());
}

#[test]
fn embedded_v1_parses() {
    let s = Schema::v1();
    assert_eq!(s.version().major, 1);
    assert!(s.root_by_name("meta").is_some());
}

#[test]
fn error_unknown_type_has_position() {
    let e = parse_err(&with_header("[model.top]\ngold = \"i33\"\n"));
    assert_eq!(e, "schema:3:8: unknown type `i33`");
}

#[test]
fn error_duplicate_field() {
    // A duplicate key in one TOML table is a TOML error with a position.
    let e = parse_err(&with_header(
        "[type.A]\nx = \"i32\"\nx = \"bool\"\n[model.top]\na = \"A\"\n",
    ));
    assert!(e.starts_with("schema:4:1:"), "{e}");
    assert!(e.contains("duplicate"), "{e}");
}

#[test]
fn error_duplicate_enum_variant() {
    let e = parse_err(&with_header("[enum]\nE = [\"a\", \"b\", \"a\"]\n"));
    assert!(e.contains("duplicate variant `a`"), "{e}");
    assert!(e.starts_with("schema:3:"), "{e}");
}

#[test]
fn error_list_key_not_a_field() {
    let e = parse_err(&with_header(
        "[type.R]\nid = \"i32\"\n[model.top]\nl = { list = \"R\", key = \"nope\" }\n",
    ));
    assert!(e.contains("list key `nope` is not a field of `R`"), "{e}");
}

#[test]
fn error_list_key_wrong_type() {
    let e = parse_err(&with_header(
        "[type.R]\nid = \"bool\"\n[model.top]\nl = { list = \"R\", key = \"id\" }\n",
    ));
    assert!(e.contains("must be `key<...>`"), "{e}");
    let e = parse_err(&with_header(
        "[type.R]\nid = \"i32?\"\n[model.top]\nl = { list = \"R\", key = \"id\" }\n",
    ));
    assert!(e.contains("must be `key<...>`"), "{e}");
}

#[test]
fn error_list_element_not_a_struct() {
    let e = parse_err(&with_header(
        "[model.top]\nl = { list = \"i32\", key = \"id\" }\n",
    ));
    assert!(e.contains("unknown list element type `i32`"), "{e}");
}

#[test]
fn error_list_without_key() {
    let e = parse_err(&with_header(
        "[type.R]\nid = \"i32\"\n[model.top]\nl = { list = \"R\" }\n",
    ));
    assert!(e.contains("needs `key"), "{e}");
}

#[test]
fn error_recursive_type() {
    let e = parse_err(&with_header(
        "[type.A]\nb = \"B?\"\n[type.B]\na = \"A?\"\n[model.top]\nx = \"A\"\n",
    ));
    assert!(e.contains("contains itself: A -> B -> A"), "{e}");
}

#[test]
fn error_nested_option() {
    let e = parse_err(&with_header("[model.top]\nx = \"i32??\"\n"));
    assert!(e.contains("option of an option"), "{e}");
}

#[test]
fn error_inline_struct_field() {
    let e = parse_err(&with_header("[model.top]\nx = { a = \"i32\" }\n"));
    assert!(e.contains("unknown attribute `a`"), "{e}");
}

#[test]
fn error_js_outside_ui() {
    let e = parse_err(&with_header(
        "[model.top]\nx = { type = \"i32\", js = true }\n",
    ));
    assert!(e.contains("allowed only below the `ui` root"), "{e}");
}

#[test]
fn error_reserved_root() {
    let e = parse_err(&with_header("[model.state]\nx = \"i32\"\n"));
    assert!(
        e.contains("reserved for the state and mod namespaces"),
        "{e}"
    );
}

#[test]
fn error_missing_version_and_bad_toml() {
    let e = parse_err("[model.top]\nx = \"i32\"\n");
    assert!(e.contains("missing `version"), "{e}");
    let e = parse_err("version = \"1.0\"\n[model.top\n");
    assert!(e.starts_with("schema:"), "{e}");
    assert!(e.contains("invalid TOML"), "{e}");
    let e = parse_err("version = \"one\"\n");
    assert!(e.contains("MAJOR.MINOR"), "{e}");
}

#[test]
fn error_bad_key_type_and_names() {
    let e = parse_err(&with_header("[model.top]\nx = \"key<>\"\n"));
    assert!(e.contains("key<"), "{e}");
    let e = parse_err(&with_header("[model.top]\nBad = \"i32\"\n"));
    assert!(e.contains("lower-case snake case"), "{e}");
    let e = parse_err(&with_header("[model.top]\n_x = \"i32\"\n"));
    assert!(e.contains("starting with `_`"), "{e}");
    let e = parse_err(&with_header("[type.lower]\nx = \"i32\"\n"));
    assert!(e.contains("CamelCase"), "{e}");
}

#[test]
fn error_intent_arguments() {
    // A plain struct is fine as an argument; one that holds a list is not.
    Schema::parse(&with_header(
        "[type.A]\nx = \"i32\"\n[intent.go]\na = \"A\"\n",
    ))
    .unwrap();
    let e = parse_err(&with_header(
        "[type.E]\nid = \"i32\"\n[type.A]\nxs = { list = \"E\", key = \"id\" }\n[intent.go]\na = \"A\"\n",
    ));
    assert!(e.contains("structs in intents must be plain"), "{e}");
    let e = parse_err(&with_header(
        "[type.E]\nid = \"i32\"\n[intent.go]\na = { list = \"E\", key = \"id\" }\n",
    ));
    assert!(e.contains("lists are not allowed in intents"), "{e}");
}

#[test]
fn list_and_optional_roots_and_per_variant_structs() {
    let s = Schema::parse(&with_header(
        r#"[enum]
Kind = ["a", "b"]
[type.Row]
id = "key<row>"
n = "i32"
[type.Cell]
on = "bool"
[type.Cells]
_each = "Kind"
_type = "Cell"
[model.rows]
_list = "Row"
_key = "id"
[model.one]
_optional = true
cells = "Cells"
"#,
    ))
    .unwrap();
    assert_eq!(s.type_name(&s.resolve("rows").unwrap().ty), "list<Row>");
    assert_eq!(s.type_name(&s.resolve("one").unwrap().ty), "One?");
    assert_eq!(s.resolve("rows[].n").unwrap().id, FieldId(1 + 1));
    assert_eq!(
        s.path(s.resolve("rows").unwrap().id).list_key.as_deref(),
        Some("id")
    );
    assert!(s.resolve("one.cells.b.on").is_ok());
    let cells: Vec<&str> = s
        .types()
        .iter()
        .find(|t| t.name == "Cells")
        .unwrap()
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(cells, ["a", "b"]);
    assert!(matches!(
        s.root(s.root_by_name("rows").unwrap()).shape,
        RootShape::List(_)
    ));

    for (src, msg) in [
        ("[model.x]\n_list = \"Row\"\n", "needs `_key"),
        ("[model.x]\n_key = \"id\"\n", "`_key` without `_list`"),
        (
            "[model.x]\n_optional = true\n_list = \"Row\"\n_key = \"id\"\n",
            "cannot be `_optional`",
        ),
        (
            "[model.x]\n_list = \"Row\"\n_key = \"id\"\na = \"i32\"\n",
            "has no fields",
        ),
        (
            "[model.x]\n_list = \"Nope\"\n_key = \"id\"\n",
            "unknown list element type",
        ),
        (
            "[model.x]\n_list = \"Row\"\n_key = \"ok\"\n",
            "must be `key<...>`",
        ),
        ("[model.x]\n_what = true\n", "starting with `_`"),
    ] {
        let e = parse_err(&with_header(&format!(
            "[type.Row]\nid = \"key<row>\"\nn = \"i32\"\nok = \"bool\"\n{src}"
        )));
        assert!(e.contains(msg), "{src}: {e}");
    }
    let e = parse_err(&with_header(
        "[enum]\nK = [\"a\"]\n[type.T]\n_each = \"K\"\n[model.x]\nn = \"T\"\n",
    ));
    assert!(e.contains("`_each` and `_type` go together"), "{e}");
    let e = parse_err(&with_header(
        "[enum]\nK = [\"a\"]\n[type.T]\n_each = \"Q\"\n_type = \"i32\"\n[model.x]\nn = \"T\"\n",
    ));
    assert!(e.contains("unknown enum `Q`"), "{e}");
}

#[test]
fn resolve_errors_explain() {
    let s = Schema::parse(REFERENCE).unwrap();
    let e = s.resolve("nope.x").unwrap_err();
    assert!(matches!(e, SchemaError::Path { .. }));
    assert_eq!(e.to_string(), "path `nope.x`: unknown root `nope`");
    let e = s.resolve("top.gld").unwrap_err().to_string();
    assert_eq!(e, "path `top.gld`: `top` has no field `gld`");
    let e = s.resolve("top.resources.name").unwrap_err().to_string();
    assert!(e.contains("is a list"), "{e}");
}

#[test]
fn version_compatibility() {
    use ui2_model::Version;
    let client = Version::parse("1.3").unwrap();
    assert!(client.accepts(Version::parse("1.0").unwrap()));
    assert!(client.accepts(Version::parse("1.3").unwrap()));
    assert!(!client.accepts(Version::parse("1.4").unwrap()));
    assert!(!client.accepts(Version::parse("2.0").unwrap()));
    assert!(Version::parse("1").is_none());
    assert!(Version::parse("1.x").is_none());
}
